//! Main TUI application
//!
//! Event-driven application that coordinates:
//! - Terminal rendering with ratatui
//! - User input handling
//! - Background state updates

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame, Terminal,
};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use super::event::{AppEvent, EventLoop, InputEvent, StateUpdate, UserCommand};
use super::widgets::{DiffView, DiffViewState, Preview, PreviewState, TreeList, TreeListState};
use crate::config::{AppState, Config};
use crate::error::{Result, TuiError};
use crate::git::DiffInfo;
use crate::session::SessionListItem;

/// Which pane is currently focused
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedPane {
    #[default]
    SessionList,
    Preview,
    Diff,
}

/// Modal dialog state
#[derive(Debug, Clone)]
pub enum Modal {
    /// No modal open
    None,
    /// Text input modal
    Input {
        title: String,
        prompt: String,
        value: String,
        on_submit: InputAction,
    },
    /// Confirmation modal
    Confirm {
        title: String,
        message: String,
        on_confirm: ConfirmAction,
    },
    /// Help modal
    Help,
}

/// Action to perform when input modal is submitted
#[derive(Debug, Clone)]
pub enum InputAction {
    CreateSession,
    AddProject,
}

/// Action to perform when confirm modal is confirmed
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteSession { session_id: crate::session::SessionId },
    RemoveProject { project_id: crate::session::ProjectId },
}

/// Application UI state
pub struct AppUiState {
    /// Session list state
    pub list_state: TreeListState,
    /// Preview pane state
    pub preview_state: PreviewState,
    /// Diff pane state
    pub diff_state: DiffViewState,
    /// Currently focused pane
    pub focused_pane: FocusedPane,
    /// Current modal
    pub modal: Modal,
    /// Session list items (flattened hierarchy)
    pub list_items: Vec<SessionListItem>,
    /// Preview content
    pub preview_content: String,
    /// Diff info
    pub diff_info: DiffInfo,
    /// Status message
    pub status_message: Option<String>,
    /// Should quit
    pub should_quit: bool,
}

impl Default for AppUiState {
    fn default() -> Self {
        Self {
            list_state: TreeListState::new(),
            preview_state: PreviewState::new(),
            diff_state: DiffViewState::new(),
            focused_pane: FocusedPane::default(),
            modal: Modal::None,
            list_items: Vec::new(),
            preview_content: String::new(),
            diff_info: DiffInfo::empty(),
            status_message: None,
            should_quit: false,
        }
    }
}

/// Main TUI application
pub struct App {
    /// Configuration
    config: Config,
    /// Application state (shared with background tasks)
    app_state: Arc<RwLock<AppState>>,
    /// UI state
    ui_state: AppUiState,
    /// Event loop
    event_loop: EventLoop,
}

impl App {
    /// Create a new application
    pub fn new(config: Config, app_state: AppState) -> Self {
        Self {
            config,
            app_state: Arc::new(RwLock::new(app_state)),
            ui_state: AppUiState::default(),
            event_loop: EventLoop::new(),
        }
    }

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Initialize terminal
        let mut terminal = self.setup_terminal()?;

        // Start event loop
        let tick_rate = Duration::from_millis(1000 / self.config.ui_refresh_fps as u64);
        self.event_loop.start(tick_rate);

        // Initial state refresh
        self.refresh_list_items().await;

        // Main loop
        let result = self.main_loop(&mut terminal).await;

        // Restore terminal
        self.restore_terminal(&mut terminal)?;

        result
    }

    /// Setup terminal for TUI
    fn setup_terminal(&self) -> Result<Terminal<CrosstermBackend<Stdout>>> {
        enable_raw_mode().map_err(|e| TuiError::InitFailed(e.to_string()))?;

        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .map_err(|e| TuiError::InitFailed(e.to_string()))?;

        let backend = CrosstermBackend::new(stdout);
        let terminal =
            Terminal::new(backend).map_err(|e| TuiError::InitFailed(e.to_string()))?;

        Ok(terminal)
    }

    /// Restore terminal to normal state
    fn restore_terminal(
        &self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        disable_raw_mode().map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )
        .map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        terminal
            .show_cursor()
            .map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        Ok(())
    }

    /// Main event loop
    async fn main_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        loop {
            // Render
            terminal
                .draw(|f| self.render(f))
                .map_err(|e| TuiError::RenderError(e.to_string()))?;

            // Handle events
            if let Some(event) = self.event_loop.next().await {
                match event {
                    AppEvent::Input(input) => self.handle_input(input).await,
                    AppEvent::StateUpdate(update) => self.handle_state_update(update).await,
                    AppEvent::Tick => {
                        // Periodic refresh if needed
                    }
                    AppEvent::Quit => {
                        self.ui_state.should_quit = true;
                    }
                }
            }

            if self.ui_state.should_quit {
                break;
            }
        }

        Ok(())
    }

    /// Render the UI
    fn render(&mut self, frame: &mut Frame) {
        let size = frame.area();

        // Main layout: session list on left, preview/diff on right
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(size);

        // Right side: preview on top, diff on bottom
        let right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(main_chunks[1]);

        // Render session list
        self.render_session_list(frame, main_chunks[0]);

        // Render preview
        self.render_preview(frame, right_chunks[0]);

        // Render diff
        self.render_diff(frame, right_chunks[1]);

        // Render modal if open
        self.render_modal(frame, size);

        // Render status bar
        self.render_status_bar(frame, size);
    }

    /// Render the session list
    fn render_session_list(&mut self, frame: &mut Frame, area: Rect) {
        let is_focused = matches!(self.ui_state.focused_pane, FocusedPane::SessionList);

        let block = Block::default()
            .title(" Sessions ")
            .borders(Borders::ALL)
            .border_style(if is_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            });

        let tree_list = TreeList::new(&self.ui_state.list_items)
            .block(block)
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(tree_list, area, &mut self.ui_state.list_state.list_state);
    }

    /// Render the preview pane
    fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        let is_focused = matches!(self.ui_state.focused_pane, FocusedPane::Preview);

        let block = Block::default()
            .title(" Preview ")
            .borders(Borders::ALL)
            .border_style(if is_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            });

        // Update preview state with visible area
        let inner_height = area.height.saturating_sub(2);
        self.ui_state
            .preview_state
            .set_content(&self.ui_state.preview_content, inner_height);

        let preview = Preview::new(&self.ui_state.preview_content)
            .block(block)
            .scroll(self.ui_state.preview_state.scroll_offset);

        frame.render_widget(preview, area);
    }

    /// Render the diff pane
    fn render_diff(&mut self, frame: &mut Frame, area: Rect) {
        let is_focused = matches!(self.ui_state.focused_pane, FocusedPane::Diff);

        let title = format!(
            " Diff ({}) ",
            self.ui_state.diff_info.summary()
        );

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if is_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            });

        // Update diff state with visible area
        let inner_height = area.height.saturating_sub(2);
        self.ui_state
            .diff_state
            .set_content(&self.ui_state.diff_info.diff, inner_height);

        let diff_view = DiffView::new(&self.ui_state.diff_info)
            .block(block)
            .scroll(self.ui_state.diff_state.scroll_offset);

        frame.render_widget(diff_view, area);
    }

    /// Render modal overlay
    fn render_modal(&self, frame: &mut Frame, area: Rect) {
        match &self.ui_state.modal {
            Modal::None => {}

            Modal::Input {
                title,
                prompt,
                value,
                ..
            } => {
                let modal_area = centered_rect(60, 20, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\n> {}_", prompt, value);
                let paragraph = Paragraph::new(text);
                frame.render_widget(paragraph, inner);
            }

            Modal::Confirm { title, message, .. } => {
                let modal_area = centered_rect(50, 15, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\n[Enter] Confirm  [Esc] Cancel", message);
                let paragraph = Paragraph::new(text);
                frame.render_widget(paragraph, inner);
            }

            Modal::Help => {
                let modal_area = centered_rect(70, 80, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(" Help ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let help_text = r#"
Navigation:
  j/k, ↑/↓    Navigate session list
  Enter       Attach to selected session
  Tab         Switch between panes

Session Management:
  n           New worktree session
  N           New project (open repo)
  p           Pause session
  r           Resume session
  d           Delete/kill session

Scrolling:
  Ctrl+u/d    Page up/down in preview
  PgUp/PgDn   Page up/down

Other:
  ?           Show this help
  q           Quit

Press any key to close this help.
"#;

                let paragraph = Paragraph::new(help_text);
                frame.render_widget(paragraph, inner);
            }
        }
    }

    /// Render status bar
    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        if area.height < 2 {
            return;
        }

        let status_area = Rect {
            x: area.x,
            y: area.height - 1,
            width: area.width,
            height: 1,
        };

        let status = if let Some(ref msg) = self.ui_state.status_message {
            msg.clone()
        } else {
            let session_count = self.ui_state.list_items.iter()
                .filter(|i| i.is_worktree())
                .count();
            format!("Sessions: {} | Press ? for help", session_count)
        };

        let paragraph = Paragraph::new(status)
            .style(Style::default().bg(Color::DarkGray));

        frame.render_widget(paragraph, status_area);
    }

    /// Handle input events
    async fn handle_input(&mut self, input: InputEvent) {
        match input {
            InputEvent::Key(key) => {
                // Check for modal-specific handling first
                if !matches!(self.ui_state.modal, Modal::None) {
                    self.handle_modal_key(key).await;
                    return;
                }

                // Convert to command and handle
                if let Some(cmd) = UserCommand::from_key(key) {
                    self.handle_command(cmd).await;
                }
            }
            InputEvent::Resize(_, _) => {
                // Terminal will re-render automatically
            }
            InputEvent::Mouse(_) => {
                // Mouse handling if needed
            }
        }
    }

    /// Handle modal key input
    async fn handle_modal_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        match &mut self.ui_state.modal {
            Modal::Input { value, on_submit, .. } => {
                match key.code {
                    KeyCode::Enter => {
                        let action = on_submit.clone();
                        let value = value.clone();
                        self.ui_state.modal = Modal::None;
                        self.handle_input_submit(action, value).await;
                    }
                    KeyCode::Esc => {
                        self.ui_state.modal = Modal::None;
                    }
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char(c) => {
                        value.push(c);
                    }
                    _ => {}
                }
            }

            Modal::Confirm { on_confirm, .. } => {
                match key.code {
                    KeyCode::Enter => {
                        let action = on_confirm.clone();
                        self.ui_state.modal = Modal::None;
                        self.handle_confirm(action).await;
                    }
                    KeyCode::Esc => {
                        self.ui_state.modal = Modal::None;
                    }
                    _ => {}
                }
            }

            Modal::Help => {
                // Any key closes help
                self.ui_state.modal = Modal::None;
            }

            Modal::None => {}
        }
    }

    /// Handle a user command
    async fn handle_command(&mut self, cmd: UserCommand) {
        match cmd {
            UserCommand::NavigateUp => {
                self.ui_state.list_state.previous();
            }
            UserCommand::NavigateDown => {
                self.ui_state.list_state.next();
            }
            UserCommand::Select => {
                self.handle_select().await;
            }
            UserCommand::NewSession => {
                self.ui_state.modal = Modal::Input {
                    title: "New Session".to_string(),
                    prompt: "Enter session name:".to_string(),
                    value: String::new(),
                    on_submit: InputAction::CreateSession,
                };
            }
            UserCommand::NewProject => {
                self.ui_state.modal = Modal::Input {
                    title: "Add Project".to_string(),
                    prompt: "Enter project path:".to_string(),
                    value: std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    on_submit: InputAction::AddProject,
                };
            }
            UserCommand::PauseSession => {
                self.handle_pause_session().await;
            }
            UserCommand::ResumeSession => {
                self.handle_resume_session().await;
            }
            UserCommand::DeleteSession => {
                self.handle_delete_session().await;
            }
            UserCommand::TogglePane => {
                self.ui_state.focused_pane = match self.ui_state.focused_pane {
                    FocusedPane::SessionList => FocusedPane::Preview,
                    FocusedPane::Preview => FocusedPane::Diff,
                    FocusedPane::Diff => FocusedPane::SessionList,
                };
            }
            UserCommand::ShowHelp => {
                self.ui_state.modal = Modal::Help;
            }
            UserCommand::Quit => {
                self.ui_state.should_quit = true;
            }
            UserCommand::PageUp => {
                match self.ui_state.focused_pane {
                    FocusedPane::Preview => self.ui_state.preview_state.page_up(),
                    FocusedPane::Diff => self.ui_state.diff_state.page_up(),
                    _ => {}
                }
            }
            UserCommand::PageDown => {
                match self.ui_state.focused_pane {
                    FocusedPane::Preview => self.ui_state.preview_state.page_down(),
                    FocusedPane::Diff => self.ui_state.diff_state.page_down(),
                    _ => {}
                }
            }
            UserCommand::ScrollUp => {
                match self.ui_state.focused_pane {
                    FocusedPane::Preview => self.ui_state.preview_state.scroll_up(1),
                    FocusedPane::Diff => self.ui_state.diff_state.scroll_up(1),
                    _ => {}
                }
            }
            UserCommand::ScrollDown => {
                match self.ui_state.focused_pane {
                    FocusedPane::Preview => self.ui_state.preview_state.scroll_down(1),
                    FocusedPane::Diff => self.ui_state.diff_state.scroll_down(1),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Handle state updates from background tasks
    async fn handle_state_update(&mut self, update: StateUpdate) {
        match update {
            StateUpdate::ContentUpdated { session_id, .. } => {
                debug!("Content updated for session {}", session_id);
                // Refresh preview if this is the selected session
            }
            StateUpdate::StatusChanged { session_id } => {
                debug!("Status changed for session {}", session_id);
                self.refresh_list_items().await;
            }
            StateUpdate::SessionAdded { session_id } => {
                debug!("Session added: {}", session_id);
                self.refresh_list_items().await;
            }
            StateUpdate::SessionRemoved { session_id } => {
                debug!("Session removed: {}", session_id);
                self.refresh_list_items().await;
            }
            StateUpdate::Error { message } => {
                self.ui_state.status_message = Some(format!("Error: {}", message));
            }
            _ => {}
        }
    }

    /// Handle selection (attach to session)
    async fn handle_select(&mut self) {
        // TODO: Implement session attachment
        self.ui_state.status_message = Some("Attach not yet implemented".to_string());
    }

    /// Handle pause session
    async fn handle_pause_session(&mut self) {
        // TODO: Implement session pause
        self.ui_state.status_message = Some("Pause not yet implemented".to_string());
    }

    /// Handle resume session
    async fn handle_resume_session(&mut self) {
        // TODO: Implement session resume
        self.ui_state.status_message = Some("Resume not yet implemented".to_string());
    }

    /// Handle delete session
    async fn handle_delete_session(&mut self) {
        // TODO: Implement session deletion with confirmation
        self.ui_state.status_message = Some("Delete not yet implemented".to_string());
    }

    /// Handle input modal submission
    async fn handle_input_submit(&mut self, action: InputAction, value: String) {
        match action {
            InputAction::CreateSession => {
                // TODO: Create session with given name
                self.ui_state.status_message =
                    Some(format!("Would create session: {}", value));
            }
            InputAction::AddProject => {
                // TODO: Add project from path
                self.ui_state.status_message =
                    Some(format!("Would add project: {}", value));
            }
        }
    }

    /// Handle confirmation
    async fn handle_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteSession { session_id } => {
                // TODO: Actually delete the session
                self.ui_state.status_message =
                    Some(format!("Would delete session: {}", session_id));
            }
            ConfirmAction::RemoveProject { project_id } => {
                // TODO: Actually remove the project
                self.ui_state.status_message =
                    Some(format!("Would remove project: {}", project_id));
            }
        }
    }

    /// Refresh the list items from app state
    async fn refresh_list_items(&mut self) {
        let state = self.app_state.read().await;

        let mut items = Vec::new();

        // Build hierarchical list
        for project in state.projects.values() {
            // Add project item
            items.push(SessionListItem::Project {
                id: project.id,
                name: project.name.clone(),
                repo_path: project.repo_path.clone(),
                main_branch: project.main_branch.clone(),
                worktree_count: project.worktrees.len(),
            });

            // Add worktree sessions for this project
            for session_id in &project.worktrees {
                if let Some(session) = state.sessions.get(session_id) {
                    items.push(SessionListItem::Worktree {
                        id: session.id,
                        project_id: session.project_id,
                        title: session.title.clone(),
                        branch: session.branch.clone(),
                        status: session.status,
                        agent_state: session.agent_state,
                        program: session.program.clone(),
                    });
                }
            }
        }

        self.ui_state.list_items = items;
        self.ui_state.list_state.set_item_count(self.ui_state.list_items.len());
    }
}

/// Helper to create a centered rect
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let centered = centered_rect(50, 50, area);

        // Should be roughly centered
        assert!(centered.x > 0);
        assert!(centered.y > 0);
        assert!(centered.width < area.width);
        assert!(centered.height < area.height);
    }

    #[test]
    fn test_app_ui_state_default() {
        let state = AppUiState::default();
        assert!(state.list_items.is_empty());
        assert!(matches!(state.focused_pane, FocusedPane::SessionList));
        assert!(matches!(state.modal, Modal::None));
        assert!(!state.should_quit);
    }
}
