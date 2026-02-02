//! Main TUI application
//!
//! Event-driven application that coordinates:
//! - Terminal rendering with ratatui
//! - User input handling
//! - Background state updates

use std::io::{self, Stdout};
use std::path::PathBuf;
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
use tracing::{debug, info};

use super::event::{AppEvent, EventLoop, InputEvent, StateUpdate, UserCommand};
use super::widgets::{DiffView, DiffViewState, Preview, PreviewState, TreeList, TreeListState};
use crate::config::{AppState, Config};
use crate::error::{Result, TuiError};
use crate::git::DiffInfo;
use crate::session::{ProjectId, SessionId, SessionListItem, SessionManager, SessionStatus};

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
    /// Error modal
    Error { message: String },
}

/// Action to perform when input modal is submitted
#[derive(Debug, Clone)]
pub enum InputAction {
    CreateSession { project_id: ProjectId },
    AddProject,
}

/// Action to perform when confirm modal is confirmed
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteSession { session_id: SessionId },
    RemoveProject { project_id: ProjectId },
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
    /// Currently selected session (for preview/diff)
    pub selected_session_id: Option<SessionId>,
    /// Currently selected project
    pub selected_project_id: Option<ProjectId>,
    /// Attach command to run after exiting TUI
    pub attach_command: Option<String>,
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
            selected_session_id: None,
            selected_project_id: None,
            attach_command: None,
        }
    }
}

/// Main TUI application
pub struct App {
    /// Configuration
    config: Config,
    /// Application state (shared with background tasks)
    app_state: Arc<RwLock<AppState>>,
    /// Session manager
    session_manager: SessionManager,
    /// UI state
    ui_state: AppUiState,
    /// Event loop
    event_loop: EventLoop,
}

impl App {
    /// Create a new application
    pub fn new(config: Config, app_state: AppState) -> Self {
        let app_state = Arc::new(RwLock::new(app_state));
        let session_manager = SessionManager::new(config.clone(), app_state.clone());

        Self {
            config,
            app_state,
            session_manager,
            ui_state: AppUiState::default(),
            event_loop: EventLoop::new(),
        }
    }

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Check tmux is available
        self.session_manager.check_tmux().await?;

        // One-time setup
        self.sync_session_states().await;
        let tick_rate = Duration::from_millis(1000 / self.config.ui_refresh_fps as u64);
        self.event_loop.start(tick_rate);
        self.start_background_updater();

        loop {
            // Setup terminal for TUI
            let mut terminal = self.setup_terminal()?;
            self.refresh_list_items().await;

            // Run main loop until quit or attach
            info!("Entering main loop");
            let result = self.main_loop(&mut terminal).await;
            info!("Main loop exited with result: {:?}", result.is_ok());

            // Restore terminal before attach or exit
            info!("Restoring terminal");
            self.restore_terminal(&mut terminal)?;
            info!("Terminal restored successfully");

            // Reset should_quit for next iteration
            self.ui_state.should_quit = false;

            match self.ui_state.attach_command.take() {
                Some(cmd) => {
                    // Attach to session (TUI is paused)
                    info!("Executing attach command: {}", cmd);
                    let session_name = cmd.split_whitespace().last().unwrap_or("");
                    if !session_name.is_empty() {
                        let _ = crate::tmux::attach_to_session(session_name).await;
                    }
                    info!("Returned from attach, resuming TUI with preserved state");
                    // Loop continues, TUI resumes with state preserved
                }
                None => break, // User quit
            }
        }

        Ok(())
    }

    /// Sync app state with actual tmux session state
    ///
    /// This method checks all active sessions and updates their status
    /// if the corresponding tmux session no longer exists or the pane is dead.
    async fn sync_session_states(&self) {
        let session_ids: Vec<(SessionId, String)> = {
            let state = self.app_state.read().await;
            state
                .sessions
                .values()
                .filter(|s| s.status.is_active())
                .map(|s| (s.id, s.tmux_session_name.clone()))
                .collect()
        };

        for (session_id, tmux_name) in session_ids {
            let should_mark_stopped = if let Ok(exists) = self.session_manager.tmux.session_exists(&tmux_name).await {
                if !exists {
                    true
                } else {
                    // Session exists, but check if pane is dead (program exited)
                    self.session_manager.tmux.is_pane_dead(&tmux_name).await.unwrap_or(false)
                }
            } else {
                false
            };

            if should_mark_stopped {
                // Kill the tmux session if it exists but pane is dead
                let _ = self.session_manager.tmux.kill_session(&tmux_name).await;

                let mut state = self.app_state.write().await;
                if let Some(session) = state.get_session_mut(&session_id) {
                    session.set_status(SessionStatus::Stopped);
                }
            }
        }

        // Save updated state
        let state = self.app_state.read().await;
        let _ = state.save();
    }

    /// Start background state updater task
    fn start_background_updater(&self) {
        let sender = self.event_loop.sender();
        let app_state = self.app_state.clone();
        let _session_manager_config = self.config.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));

            loop {
                interval.tick().await;

                // Get active session IDs
                let session_ids: Vec<SessionId> = {
                    let state = app_state.read().await;
                    state.get_active_sessions().iter().map(|s| s.id).collect()
                };

                // Update agent states (this is done in session manager)
                // For now, just signal a state update
                if !session_ids.is_empty() {
                    let _ = sender.send(AppEvent::Tick).await;
                }
            }
        });
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
        info!("Disabling raw mode");
        disable_raw_mode().map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        info!("Leaving alternate screen and disabling mouse capture");
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )
        .map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        info!("Showing cursor");
        terminal
            .show_cursor()
            .map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        info!("Terminal restore complete");
        Ok(())
    }

    /// Main event loop
    async fn main_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        loop {
            // Update selection tracking
            self.update_selection();

            // Update preview for selected session
            self.update_preview().await;

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
                        // Refresh state periodically
                        self.refresh_list_items().await;
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

    /// Update selection tracking based on list position
    fn update_selection(&mut self) {
        if let Some(idx) = self.ui_state.list_state.selected() {
            if let Some(item) = self.ui_state.list_items.get(idx) {
                match item {
                    SessionListItem::Project { id, .. } => {
                        self.ui_state.selected_project_id = Some(*id);
                        self.ui_state.selected_session_id = None;
                    }
                    SessionListItem::Worktree { id, project_id, .. } => {
                        self.ui_state.selected_session_id = Some(*id);
                        self.ui_state.selected_project_id = Some(*project_id);
                    }
                }
            }
        }
    }

    /// Update preview pane content
    async fn update_preview(&mut self) {
        if let Some(session_id) = self.ui_state.selected_session_id {
            // Get content
            match self.session_manager.get_content(&session_id).await {
                Ok(content) => {
                    self.ui_state.preview_content = content.content;
                }
                Err(_) => {
                    self.ui_state.preview_content = "Unable to capture content".to_string();
                }
            }

            // Get diff
            match self.session_manager.get_diff(&session_id).await {
                Ok(diff) => {
                    self.ui_state.diff_info = diff;
                }
                Err(_) => {
                    self.ui_state.diff_info = DiffInfo::empty();
                }
            }
        } else {
            self.ui_state.preview_content = "Select a session to see preview".to_string();
            self.ui_state.diff_info = DiffInfo::empty();
        }
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

            Modal::Error { message } => {
                let modal_area = centered_rect(60, 20, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(" Error ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\nPress any key to close.", message);
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
  j/k, Up/Down    Navigate session list
  Enter           Attach to selected session
  Tab             Switch between panes

Session Management:
  n               New worktree session (under selected project)
  N               New project (add git repo)
  p               Pause session
  r               Resume session
  d               Delete/kill session

Scrolling:
  Ctrl+u/d        Page up/down in preview
  PgUp/PgDn       Page up/down

Other:
  ?               Show this help
  q               Quit

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
            format!("Sessions: {} | Press ? for help | n: new session | N: add project", session_count)
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

            Modal::Help | Modal::Error { .. } => {
                // Any key closes help/error
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
                self.handle_new_session();
            }
            UserCommand::NewProject => {
                self.ui_state.modal = Modal::Input {
                    title: "Add Project".to_string(),
                    prompt: "Enter path to git repository:".to_string(),
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
                self.handle_delete_session();
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
                self.ui_state.modal = Modal::Error { message };
            }
            _ => {}
        }
    }

    /// Handle selection (attach to session)
    async fn handle_select(&mut self) {
        info!("handle_select called, selected_session_id: {:?}", self.ui_state.selected_session_id);
        if let Some(session_id) = self.ui_state.selected_session_id {
            info!("Getting attach command for session: {}", session_id);
            match self.session_manager.get_attach_command(&session_id).await {
                Ok(cmd) => {
                    info!("Got attach command: {}", cmd);
                    self.ui_state.attach_command = Some(cmd);
                    self.ui_state.should_quit = true;
                    info!("Set should_quit = true");
                }
                Err(e) => {
                    info!("Failed to get attach command: {}", e);
                    self.ui_state.modal = Modal::Error {
                        message: format!("Cannot attach: {}", e),
                    };
                }
            }
        } else {
            info!("No session selected");
        }
    }

    /// Handle new session command
    fn handle_new_session(&mut self) {
        if let Some(project_id) = self.ui_state.selected_project_id {
            self.ui_state.modal = Modal::Input {
                title: "New Session".to_string(),
                prompt: "Enter session name:".to_string(),
                value: String::new(),
                on_submit: InputAction::CreateSession { project_id },
            };
        } else {
            self.ui_state.status_message = Some("Select a project first (use N to add one)".to_string());
        }
    }

    /// Handle pause session
    async fn handle_pause_session(&mut self) {
        if let Some(session_id) = self.ui_state.selected_session_id {
            match self.session_manager.pause_session(&session_id).await {
                Ok(_) => {
                    self.ui_state.status_message = Some("Session paused".to_string());
                    self.refresh_list_items().await;
                }
                Err(e) => {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Failed to pause: {}", e),
                    };
                }
            }
        }
    }

    /// Handle resume session
    async fn handle_resume_session(&mut self) {
        if let Some(session_id) = self.ui_state.selected_session_id {
            match self.session_manager.resume_session(&session_id).await {
                Ok(_) => {
                    self.ui_state.status_message = Some("Session resumed".to_string());
                    self.refresh_list_items().await;
                }
                Err(e) => {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Failed to resume: {}", e),
                    };
                }
            }
        }
    }

    /// Handle delete session - show confirmation
    fn handle_delete_session(&mut self) {
        if let Some(session_id) = self.ui_state.selected_session_id {
            self.ui_state.modal = Modal::Confirm {
                title: "Delete Session".to_string(),
                message: "Are you sure you want to delete this session?\nThis will kill the tmux session and remove the worktree.".to_string(),
                on_confirm: ConfirmAction::DeleteSession { session_id },
            };
        }
    }

    /// Handle input modal submission
    async fn handle_input_submit(&mut self, action: InputAction, value: String) {
        match action {
            InputAction::CreateSession { project_id } => {
                if value.trim().is_empty() {
                    self.ui_state.status_message = Some("Session name cannot be empty".to_string());
                    return;
                }

                match self.session_manager.create_session(&project_id, value, None).await {
                    Ok(session_id) => {
                        self.ui_state.status_message = Some(format!("Created session {}", session_id));
                        self.refresh_list_items().await;
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to create session: {}", e),
                        };
                    }
                }
            }
            InputAction::AddProject => {
                let path = PathBuf::from(value.trim());
                if !path.exists() {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Path does not exist: {}", path.display()),
                    };
                    return;
                }

                match self.session_manager.add_project(path).await {
                    Ok(project_id) => {
                        self.ui_state.status_message = Some(format!("Added project {}", project_id));
                        self.refresh_list_items().await;
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to add project: {}", e),
                        };
                    }
                }
            }
        }
    }

    /// Handle confirmation
    async fn handle_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteSession { session_id } => {
                match self.session_manager.delete_session(&session_id).await {
                    Ok(_) => {
                        self.ui_state.status_message = Some("Session deleted".to_string());
                        self.ui_state.selected_session_id = None;
                        self.refresh_list_items().await;
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to delete: {}", e),
                        };
                    }
                }
            }
            ConfirmAction::RemoveProject { project_id } => {
                match self.session_manager.remove_project(&project_id).await {
                    Ok(_) => {
                        self.ui_state.status_message = Some("Project removed".to_string());
                        self.ui_state.selected_project_id = None;
                        self.refresh_list_items().await;
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to remove: {}", e),
                        };
                    }
                }
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

        // Clear status message after a bit
        // (In a real app, you'd use a timer)
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
