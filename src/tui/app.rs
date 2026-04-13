//! Main TUI application
//!
//! Event-driven application that coordinates:
//! - Terminal rendering with ratatui
//! - User input handling
//! - Background state updates

use std::collections::HashMap;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use tracing::{debug, info, warn};

use super::event::{AppEvent, EventLoop, InputEvent, StateUpdate, UserCommand};
use super::path_completer::PathCompleter;
use super::theme::Theme;
use super::widgets::{
    InfoContent, InfoProjectData, InfoSessionData, InfoView, InfoViewState, Preview, PreviewState,
    TreeList, TreeListState,
};
use crate::config::{BindableAction, Config, ConfigStore, StateStore};
use crate::error::{Result, TuiError};
use crate::git::{
    AiSummary, DiffInfo, EnrichedPrInfo, check_pr_for_branch, diff_hash, fetch_branch_summary,
    fetch_enriched_pr, is_gh_available,
};
use crate::session::{
    AgentState, ProjectId, SessionId, SessionListItem, SessionManager, SessionStatus,
};
use crate::tmux::AgentStateDetector;

/// Direction for mouse scroll events
enum ScrollDirection {
    Up,
    Down,
}

/// Minimum left pane width as a percentage of the content area
const MIN_LEFT_PANE_PCT: u16 = 15;
/// Maximum left pane width as a percentage of the content area
const MAX_LEFT_PANE_PCT: u16 = 60;
/// Default left pane width as a percentage of the content area
const DEFAULT_LEFT_PANE_PCT: u16 = 30;

/// Which pane is currently focused
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedPane {
    #[default]
    SessionList,
    RightPane,
}

/// Which view is shown in the right pane
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightPaneView {
    #[default]
    Preview,
    Info,
    Shell,
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
    /// Path input modal with tab completion
    PathInput {
        title: String,
        prompt: String,
        value: String,
        on_submit: InputAction,
        completer: PathCompleter,
    },
    /// Loading spinner modal (non-interactive)
    Loading { title: String, message: String },
    /// Help modal
    Help,
    /// Error modal
    Error { message: String },
    /// Settings modal
    Settings(SettingsState),
    /// Quick-switch session search modal
    QuickSwitch {
        query: String,
        matches: Vec<QuickSwitchMatch>,
        selected_idx: usize,
    },
}

/// A session match in the quick-switch modal
#[derive(Debug, Clone)]
pub struct QuickSwitchMatch {
    pub session_id: SessionId,
    pub title: String,
    pub branch: String,
    pub project_name: String,
    pub status: SessionStatus,
}

/// Which tab is active in the settings modal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    General,
    Keybindings,
    Theme,
}

impl SettingsTab {
    const ALL: [SettingsTab; 3] = [Self::General, Self::Keybindings, Self::Theme];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Keybindings => "Keybindings",
            Self::Theme => "Theme",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::General => Self::Keybindings,
            Self::Keybindings => Self::Theme,
            Self::Theme => Self::General,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::General => Self::Theme,
            Self::Keybindings => Self::General,
            Self::Theme => Self::Keybindings,
        }
    }
}

/// State for the settings modal
#[derive(Debug, Clone)]
pub struct SettingsState {
    pub tab: SettingsTab,
    pub selected_row: usize,
    pub editing: Option<SettingsEditing>,
    /// Cached row data for the current tab
    pub rows: Vec<SettingsRow>,
}

/// A single row in the settings list
#[derive(Debug, Clone)]
pub struct SettingsRow {
    pub label: String,
    pub value: String,
    pub field_key: String,
    /// Optional color for displaying a swatch next to the value (Theme tab only)
    pub color_swatch: Option<Color>,
}

/// Editing state within the settings modal
#[derive(Debug, Clone)]
pub enum SettingsEditing {
    /// Editing a text value
    TextInput { value: String },
    /// Capturing a key for keybinding
    KeyCapture {
        action_name: String,
        keys: Vec<String>,
    },
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
    RestartSession { session_id: SessionId },
    RemoveProject { project_id: ProjectId },
}

/// Application UI state
pub struct AppUiState {
    /// Session list state
    pub list_state: TreeListState,
    /// Preview pane state
    pub preview_state: PreviewState,
    /// Info pane state
    pub info_state: InfoViewState,
    /// Enriched PR info for the currently selected session
    pub enriched_pr: Option<(SessionId, EnrichedPrInfo)>,
    /// Cached AI summaries keyed by session ID
    pub ai_summaries: std::collections::HashMap<SessionId, AiSummary>,
    /// Currently focused pane
    pub focused_pane: FocusedPane,
    /// Which view is shown in the right pane
    pub right_pane_view: RightPaneView,
    /// Current modal
    pub modal: Modal,
    /// Session list items (flattened hierarchy)
    pub list_items: Vec<SessionListItem>,
    /// Preview content
    pub preview_content: String,
    /// Shell pane state
    pub shell_state: PreviewState,
    /// Shell content
    pub shell_content: String,
    /// Diff info
    pub diff_info: Arc<DiffInfo>,
    /// Status message (with expiry time)
    pub status_message: Option<(String, Instant)>,
    /// Should quit
    pub should_quit: bool,
    /// Last known terminal size (updated each render frame)
    pub terminal_size: Rect,
    /// Currently selected session (for preview/diff)
    pub selected_session_id: Option<SessionId>,
    /// Currently selected project
    pub selected_project_id: Option<ProjectId>,
    /// Attach command to run after exiting TUI
    pub attach_command: Option<String>,
    /// Editor command + path to open after exiting TUI
    pub editor_command: Option<(String, PathBuf)>,
    /// When attached via shell toggle (Ctrl+\), stores the session name to switch back to.
    /// Contains (current_session_name, paired_session_name) so we can toggle between them.
    pub shell_toggle_pair: Option<(String, String)>,
    /// Needs right pane clear (set on view switch, consumed on render)
    pub clear_right_pane: bool,
    /// Left pane width as a percentage (adjustable at runtime via < / >)
    pub left_pane_pct: u16,
    /// When the last PR status check was performed
    pub last_pr_check: Option<Instant>,
    /// Whether the `gh` CLI is available
    pub gh_available: bool,
    /// When the last background preview fetch was spawned (None = not in flight)
    pub preview_update_spawned_at: Option<Instant>,
    /// Tick counter for animations (incremented each render tick)
    pub tick_count: u64,
    /// Throbber/spinner state for loading modals
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    /// Current agent states for Running Claude sessions (ephemeral, from background poller)
    pub agent_states: HashMap<SessionId, AgentState>,
}

impl Default for AppUiState {
    fn default() -> Self {
        Self {
            list_state: TreeListState::new(),
            preview_state: PreviewState::new(),
            info_state: InfoViewState::new(),
            enriched_pr: None,
            ai_summaries: std::collections::HashMap::new(),
            shell_state: PreviewState::new(),
            shell_content: String::new(),
            focused_pane: FocusedPane::default(),
            right_pane_view: RightPaneView::default(),
            modal: Modal::None,
            list_items: Vec::new(),
            preview_content: String::new(),
            diff_info: Arc::new(DiffInfo::empty()),
            status_message: None, // (message, expiry)

            should_quit: false,
            selected_session_id: None,
            selected_project_id: None,
            attach_command: None,
            editor_command: None,
            shell_toggle_pair: None,
            clear_right_pane: false,
            left_pane_pct: DEFAULT_LEFT_PANE_PCT,
            last_pr_check: None,
            gh_available: false,
            preview_update_spawned_at: None,
            terminal_size: Rect::default(),
            tick_count: 0,
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            agent_states: HashMap::new(),
        }
    }
}

/// Main TUI application
pub struct App {
    /// Local config cache — refreshed from config_store on tick when file changes
    config: Config,
    /// Shared config store (hot-reloaded from disk)
    config_store: Arc<ConfigStore>,
    /// Concurrent-safe persistent state store
    store: Arc<StateStore>,
    /// Session manager
    session_manager: SessionManager,
    /// UI state
    ui_state: AppUiState,
    /// Event loop
    event_loop: EventLoop,
    /// Theme configuration
    theme: Theme,
    /// Suppress key events until this instant (filters stray bytes from
    /// unrecognized escape sequences that crossterm splits into multiple events)
    suppress_keys_until: Instant,
    /// Two-digit session number accumulator with debounce
    digit_accumulator: super::digit_accumulator::DigitAccumulator,
}

impl App {
    /// Create a new application
    pub fn new(config_store: Arc<ConfigStore>, store: Arc<StateStore>) -> Self {
        let config = config_store.read().clone();
        let theme = Theme::default();
        let session_manager = SessionManager::new(
            config_store.clone(),
            store.clone(),
            theme.tmux_status_style(),
        );

        let base = config
            .theme
            .preset
            .as_deref()
            .and_then(Theme::from_preset)
            .unwrap_or_default();
        let theme = base.with_overrides(&config.theme);
        let debounce = Duration::from_millis(config.session_number_debounce_ms);

        Self {
            config,
            config_store,
            store,
            session_manager,
            ui_state: AppUiState::default(),
            event_loop: EventLoop::new(),
            theme,
            suppress_keys_until: Instant::now(),
            digit_accumulator: super::digit_accumulator::DigitAccumulator::new(debounce),
        }
    }

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Check tmux is available
        self.session_manager.check_tmux().await?;

        // One-time setup
        self.cleanup_stale_creating_sessions().await;
        self.sync_session_states().await;

        // Check gh availability and do initial PR check
        if self.config.pr_check_interval_secs > 0 {
            self.ui_state.gh_available = is_gh_available().await;
            if self.ui_state.gh_available {
                self.spawn_pr_status_check();
            }
        }

        let tick_rate = Duration::from_millis(1000 / self.config.ui_refresh_fps as u64);
        self.event_loop.start(tick_rate);

        // Start background state sync for cross-instance changes
        if self.config.state_sync_interval_ms > 0 {
            let store = self.store.clone();
            let tx = self.event_loop.sender();
            let interval_ms = self.config.state_sync_interval_ms;
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
                loop {
                    interval.tick().await;
                    match store.reload_if_changed().await {
                        Ok(true) => {
                            let _ = tx
                                .send(AppEvent::StateUpdate(StateUpdate::ExternalChange))
                                .await;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            debug!("State sync check failed: {}", e);
                        }
                    }
                }
            });
        }

        // Start background agent state polling
        if self.config.agent_state_poll_interval_ms > 0 {
            let store = self.store.clone();
            let tx = self.event_loop.sender();
            let interval_ms = self.config.agent_state_poll_interval_ms;
            let tmux = self.session_manager.tmux.clone();
            tokio::spawn(async move {
                let cache_ttl = Duration::from_millis(interval_ms.saturating_sub(500).max(500));
                let mut detector = AgentStateDetector::new(tmux, cache_ttl);
                let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
                loop {
                    interval.tick().await;
                    let sessions: Vec<(SessionId, String, String)> = {
                        let state = store.read().await;
                        state
                            .sessions
                            .values()
                            .filter(|s| s.status == SessionStatus::Running)
                            .map(|s| (s.id, s.tmux_session_name.clone(), s.program.clone()))
                            .collect()
                    };
                    if sessions.is_empty() {
                        continue;
                    }
                    let states: HashMap<SessionId, AgentState> =
                        detector.detect_all(&sessions).await;
                    if !states.is_empty() {
                        let _ = tx
                            .send(AppEvent::StateUpdate(StateUpdate::AgentStatesUpdated {
                                states,
                            }))
                            .await;
                    }
                }
            });
        }

        // Restore last selection from persisted state
        self.refresh_list_items().await;
        self.restore_selection().await;

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

            if let Some((editor, path)) = self.ui_state.editor_command.take() {
                // Run editor as a foreground process, then return to TUI
                self.event_loop.stop_input();
                tokio::time::sleep(Duration::from_millis(100)).await;

                info!("Launching editor: {} {}", editor, path.display());
                let status = std::process::Command::new(&editor).arg(&path).status();

                if let Err(e) = status {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Failed to launch '{}': {}", editor, e),
                    };
                }

                self.event_loop.restart_input();
            } else {
                match self.ui_state.attach_command.take() {
                    Some(cmd) => {
                        // Stop the input reader BEFORE attaching so it doesn't compete for stdin
                        self.event_loop.stop_input();
                        // Brief delay to let the reader task actually stop
                        tokio::time::sleep(Duration::from_millis(100)).await;

                        // Flush any pending input (e.g. the Enter key that triggered this attach)
                        crate::tmux::flush_stdin();

                        // Attach to session via async PTY bridge (supports Ctrl+Q detach, Ctrl+\ shell toggle)
                        info!("Executing attach command: {}", cmd);
                        let session_name = cmd.split_whitespace().last().unwrap_or("").to_string();
                        if !session_name.is_empty() {
                            let mut current_session = session_name.clone();

                            loop {
                                let editor_triggers =
                                    crate::config::keybindings::editor_trigger_bytes(
                                        &self.config.keybindings,
                                    );
                                match crate::tmux::attach_to_session(
                                    &current_session,
                                    editor_triggers,
                                )
                                .await
                                {
                                    Ok(crate::tmux::AttachResult::SwitchToShell) => {
                                        info!(
                                            "Shell toggle requested from session: {}",
                                            current_session
                                        );

                                        // Determine the paired session to switch to
                                        let next_session = match &self.ui_state.shell_toggle_pair {
                                            Some((_, paired)) => paired.clone(),
                                            None => {
                                                // First toggle — resolve the shell session
                                                match self
                                                    .resolve_shell_toggle_pair(&current_session)
                                                    .await
                                                {
                                                    Ok(paired) => paired,
                                                    Err(e) => {
                                                        warn!(
                                                            "Failed to resolve shell session: {}",
                                                            e
                                                        );
                                                        self.ui_state.modal = Modal::Error {
                                                            message: format!(
                                                                "Cannot switch to shell: {}",
                                                                e
                                                            ),
                                                        };
                                                        break;
                                                    }
                                                }
                                            }
                                        };

                                        // Update the toggle pair so next Ctrl+\ switches back
                                        self.ui_state.shell_toggle_pair =
                                            Some((next_session.clone(), current_session.clone()));
                                        current_session = next_session;

                                        // Flush between switches
                                        crate::tmux::flush_stdin();
                                        info!("Switching to session: {}", current_session);
                                        continue;
                                    }
                                    Ok(crate::tmux::AttachResult::OpenEditor) => {
                                        info!(
                                            "OpenEditor requested from session: {}",
                                            current_session
                                        );
                                        // Run the editor for the session's worktree, keep
                                        // the tmux session alive, and then re-attach.
                                        self.open_editor_for_tmux_session(&current_session)
                                            .await;
                                        crate::tmux::flush_stdin();
                                        continue;
                                    }
                                    Ok(result) => {
                                        info!("Attach ended: {:?}", result);
                                        // Clear toggle state on normal detach
                                        self.ui_state.shell_toggle_pair = None;
                                        break;
                                    }
                                    Err(e) => {
                                        warn!("Failed to attach to session: {}", e);
                                        self.ui_state.modal = Modal::Error {
                                            message: format!("Failed to attach: {}", e),
                                        };
                                        self.ui_state.shell_toggle_pair = None;
                                        break;
                                    }
                                }
                            }
                        }

                        // Flush stdin again after detach to discard any stale input
                        crate::tmux::flush_stdin();

                        // Restart the input reader after detach
                        info!("Returned from attach, restarting input reader");
                        self.event_loop.restart_input();
                        // Loop continues, TUI resumes with state preserved
                    }
                    None => {
                        // Save selection before quitting
                        self.save_selection().await;
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Remove any sessions stuck in `Creating` status from a previous crash.
    async fn cleanup_stale_creating_sessions(&self) {
        let creating_ids: Vec<SessionId> = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .filter(|s| s.status == SessionStatus::Creating)
                .map(|s| s.id)
                .collect()
        };

        if !creating_ids.is_empty() {
            warn!(
                "Cleaning up {} stale Creating session(s) from previous run",
                creating_ids.len()
            );
            let _ = self
                .store
                .mutate(move |state| {
                    for sid in &creating_ids {
                        state.remove_session(sid);
                    }
                })
                .await;
        }
    }

    /// Sync app state with actual tmux session state
    ///
    /// This method checks all active sessions and updates their status
    /// if the corresponding tmux session no longer exists or the pane is dead.
    async fn sync_session_states(&self) {
        let session_ids: Vec<(SessionId, String)> = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .filter(|s| s.status.is_active() && s.status != SessionStatus::Creating)
                .map(|s| (s.id, s.tmux_session_name.clone()))
                .collect()
        };

        for (session_id, tmux_name) in session_ids {
            let should_mark_stopped =
                if let Ok(exists) = self.session_manager.tmux.session_exists(&tmux_name).await {
                    if !exists {
                        true
                    } else {
                        // Session exists, but check if pane is dead (program exited)
                        self.session_manager
                            .tmux
                            .is_pane_dead(&tmux_name)
                            .await
                            .unwrap_or(false)
                    }
                } else {
                    false
                };

            if should_mark_stopped {
                // Kill the tmux session if it exists but pane is dead
                let _ = self.session_manager.tmux.kill_session(&tmux_name).await;

                let _ = self
                    .store
                    .mutate(move |state| {
                        if let Some(session) = state.get_session_mut(&session_id) {
                            session.set_status(SessionStatus::Stopped);
                        }
                    })
                    .await;
            }
        }

        // Sync unmanaged worktrees for all projects
        let project_ids: Vec<ProjectId> = {
            let state = self.store.read().await;
            state.projects.keys().copied().collect()
        };
        for project_id in project_ids {
            if let Err(e) = self.session_manager.sync_worktrees(&project_id).await {
                debug!("Failed to sync worktrees for project {}: {}", project_id, e);
            }
        }
    }

    /// Setup terminal for TUI
    fn setup_terminal(&self) -> Result<Terminal<CrosstermBackend<Stdout>>> {
        enable_raw_mode().map_err(|e| TuiError::InitFailed(e.to_string()))?;

        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        )
        .map_err(|e| TuiError::InitFailed(e.to_string()))?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).map_err(|e| TuiError::InitFailed(e.to_string()))?;

        Ok(terminal)
    }

    /// Restore terminal to normal state
    fn restore_terminal(&self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        info!("Disabling raw mode");
        disable_raw_mode().map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        info!("Leaving alternate screen");
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableBracketedPaste,
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
    async fn main_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        // Kick off an initial background preview fetch
        self.update_selection();
        self.spawn_preview_update();

        loop {
            // Force full terminal redraw on view switch to clear stale styled cells
            if self.ui_state.clear_right_pane {
                terminal
                    .clear()
                    .map_err(|e| TuiError::RenderError(e.to_string()))?;
                self.ui_state.clear_right_pane = false;
            }

            // Render with whatever data we have — never blocks on I/O
            terminal
                .draw(|f| self.render(f))
                .map_err(|e| TuiError::RenderError(e.to_string()))?;

            // Wait for at least one event
            let Some(event) = self.event_loop.next().await else {
                break;
            };

            // Process first event, then drain all pending events.
            // This ensures rapid keypresses are handled immediately
            // without waiting for the next render cycle.
            let mut needs_tick = false;
            needs_tick |= self.process_event(event).await;

            while let Some(event) = self.event_loop.try_next() {
                needs_tick |= self.process_event(event).await;
            }

            // Periodic background work (only on Tick)
            if needs_tick {
                self.refresh_list_items().await;

                // Spawn non-blocking preview update
                self.spawn_preview_update();

                // Periodic PR status check
                if self.ui_state.gh_available && self.config.pr_check_interval_secs > 0 {
                    let interval = Duration::from_secs(self.config.pr_check_interval_secs);
                    let should_check = self
                        .ui_state
                        .last_pr_check
                        .is_none_or(|t| t.elapsed() >= interval);
                    if should_check {
                        self.spawn_pr_status_check();
                    }
                }
            }

            if self.ui_state.should_quit {
                break;
            }
        }

        Ok(())
    }

    /// Process a single event, returns true if it was a Tick
    async fn process_event(&mut self, event: AppEvent) -> bool {
        match event {
            AppEvent::Input(input) => {
                let old_session = self.ui_state.selected_session_id;
                let old_project = self.ui_state.selected_project_id;

                self.handle_input(input).await;
                // Keep selection IDs in sync after input (needed for
                // correct behavior when draining multiple events)
                self.update_selection();

                // Immediately fetch preview when selection changes
                if self.ui_state.selected_session_id != old_session
                    || self.ui_state.selected_project_id != old_project
                {
                    // Cancel any in-flight fetch for the old selection
                    self.ui_state.preview_update_spawned_at = None;
                    self.spawn_preview_update();
                }
            }
            AppEvent::StateUpdate(update) => self.handle_state_update(update).await,
            AppEvent::Tick => {
                self.ui_state.tick_count = self.ui_state.tick_count.wrapping_add(1);
                if self.ui_state.tick_count.is_multiple_of(3) {
                    self.ui_state.throbber_state.calc_next();
                }

                // Resolve pending digit jump if debounce window expired
                if self.config.show_session_numbers
                    && let Some(super::digit_accumulator::DigitResult::Jump(n)) =
                        self.digit_accumulator.tick()
                {
                    self.jump_to_session_number(n);
                }

                // Check for config file changes roughly once per second
                // (tick_count wraps at u64::MAX, is_multiple_of(30) at 30fps ≈ 1s)
                if self.ui_state.tick_count.is_multiple_of(30) {
                    self.check_config_reload();
                }
                return true;
            }
            AppEvent::Quit => {
                self.ui_state.should_quit = true;
            }
        }
        false
    }

    /// Check if `config.toml` has been modified externally and refresh the local cache.
    fn check_config_reload(&mut self) {
        match self.config_store.reload_if_changed() {
            Ok(true) => {
                debug!("Config hot-reloaded from disk");
                self.config = self.config_store.read().clone();
                let base = self
                    .config
                    .theme
                    .preset
                    .as_deref()
                    .and_then(Theme::from_preset)
                    .unwrap_or_default();
                self.theme = base.with_overrides(&self.config.theme);
            }
            Ok(false) => {}
            Err(e) => {
                debug!("Config reload check failed: {}", e);
            }
        }
    }

    /// Spawn a background task to fetch preview/diff/shell data.
    ///
    /// The task runs in parallel with the main event loop so that
    /// keyboard input is never blocked by I/O. Results arrive as
    /// `StateUpdate::PreviewReady` events.
    fn spawn_preview_update(&mut self) {
        // Skip if a fetch is already in flight (with 5s safety timeout)
        if let Some(spawned_at) = self.ui_state.preview_update_spawned_at {
            if spawned_at.elapsed() < Duration::from_secs(5) {
                return;
            }
            debug!("Preview update stale (>5s), spawning new one");
        }

        let session_id = self.ui_state.selected_session_id;
        let project_id = self.ui_state.selected_project_id;
        let mgr = self.session_manager.clone();
        let tx = self.event_loop.sender();

        self.ui_state.preview_update_spawned_at = Some(Instant::now());

        debug!(
            "Spawning preview update for session={:?} project={:?}",
            session_id, project_id
        );

        tokio::spawn(async move {
            let (preview_content, diff_info, shell_content) =
                fetch_preview_data(&mgr, session_id, project_id).await;

            debug!(
                "Preview fetch complete, sending PreviewReady (preview_len={} diff_lines={})",
                preview_content.len(),
                diff_info.line_count
            );

            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::PreviewReady {
                    session_id,
                    project_id,
                    preview_content,
                    diff_info,
                    shell_content,
                }))
                .await;
        });
    }

    /// Update selection tracking based on list position
    fn update_selection(&mut self) {
        let old_session = self.ui_state.selected_session_id;
        let was_on_project = old_session.is_none() && self.ui_state.selected_project_id.is_some();

        if let Some(idx) = self.ui_state.list_state.selected()
            && let Some(item) = self.ui_state.list_items.get(idx)
        {
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

        let now_on_project = self.ui_state.selected_session_id.is_none()
            && self.ui_state.selected_project_id.is_some();

        // Auto-switch pane when transitioning between project and session
        if now_on_project && !was_on_project {
            // Transitioning to a project: Preview → Shell
            if self.ui_state.right_pane_view == RightPaneView::Preview {
                self.ui_state.right_pane_view = RightPaneView::Shell;
                self.ui_state.clear_right_pane = true;
            }
        } else if !now_on_project && was_on_project {
            // Transitioning to a session: Shell → Preview
            if self.ui_state.right_pane_view == RightPaneView::Shell {
                self.ui_state.right_pane_view = RightPaneView::Preview;
                self.ui_state.clear_right_pane = true;
            }
        }

        // Fetch info pane data if applicable
        self.spawn_info_fetch();
    }

    /// Get mutable reference to the active pane's scroll state
    fn active_pane_state(&mut self) -> &mut PreviewState {
        match self.ui_state.right_pane_view {
            RightPaneView::Preview => &mut self.ui_state.preview_state,
            RightPaneView::Info => &mut self.ui_state.info_state,
            RightPaneView::Shell => &mut self.ui_state.shell_state,
        }
    }

    /// Scroll the pane under the given mouse column position
    fn scroll_pane_at(&mut self, col: u16, direction: ScrollDirection) {
        let size = self.ui_state.terminal_size;
        if size.width == 0 || size.height == 0 {
            return;
        }

        // Recompute the same content_area as render()
        let content_area = Rect {
            x: size.x + 1,
            y: size.y + 1,
            width: size.width.saturating_sub(2),
            height: size.height.saturating_sub(3),
        };

        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(self.ui_state.left_pane_pct),
                Constraint::Percentage(100 - self.ui_state.left_pane_pct),
            ])
            .split(content_area);

        let lines_per_tick: u16 = 3;

        if col < main_chunks[0].right() {
            // Left pane: scroll the session list selection
            match direction {
                ScrollDirection::Up => self.ui_state.list_state.previous(),
                ScrollDirection::Down => self.ui_state.list_state.next(),
            }
            self.update_selection();
        } else {
            // Right pane: scroll content
            match direction {
                ScrollDirection::Up => self.active_pane_state().scroll_up(lines_per_tick),
                ScrollDirection::Down => self.active_pane_state().scroll_down(lines_per_tick),
            }
        }
    }

    /// Render the UI
    fn render(&mut self, frame: &mut Frame) {
        let size = frame.area();
        self.ui_state.terminal_size = size;

        // Content area with margin on top, left, right, and space for status bar at bottom
        let content_area = Rect {
            x: size.x + 1,
            y: size.y + 1,
            width: size.width.saturating_sub(2),
            height: size.height.saturating_sub(3), // 1 top margin + 1 bottom margin + 1 status bar
        };

        // Main layout: session list on left, right pane fills rest
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(self.ui_state.left_pane_pct),
                Constraint::Percentage(100 - self.ui_state.left_pane_pct),
            ])
            .split(content_area);

        // Render session list
        self.render_session_list(frame, main_chunks[0]);

        // Render either preview, diff, or shell based on current view
        // Defensive: if a project is selected and view is Preview, render Shell instead
        let view = if self.is_project_selected()
            && self.ui_state.right_pane_view == RightPaneView::Preview
        {
            RightPaneView::Shell
        } else {
            self.ui_state.right_pane_view
        };
        match view {
            RightPaneView::Preview => self.render_preview(frame, main_chunks[1]),
            RightPaneView::Info => self.render_info(frame, main_chunks[1]),
            RightPaneView::Shell => self.render_shell(frame, main_chunks[1]),
        }

        // Render modal if open
        self.render_modal(frame, content_area);

        // Render status bar at the very bottom of the screen
        self.render_status_bar(frame, size);
    }

    /// Render the session list
    fn render_session_list(&mut self, frame: &mut Frame, area: Rect) {
        // Split into a 1-line heading bar and the list below
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        // Full-width heading bar with dark grey background
        let heading_style = self.theme.status_bar();
        let heading =
            Paragraph::new(Line::styled(" Sessions:", heading_style)).style(heading_style);
        frame.render_widget(heading, chunks[0]);

        let tree_list = TreeList::new(&self.ui_state.list_items, &self.theme)
            .show_numbers(self.config.show_session_numbers)
            .tick(self.ui_state.tick_count)
            .highlight_style(self.theme.selection().add_modifier(Modifier::BOLD))
            .review_labels(&self.config.pr_review_labels)
            .invert_pr_label_color(self.config.invert_pr_label_color);

        frame.render_stateful_widget(
            tree_list,
            chunks[1],
            &mut self.ui_state.list_state.list_state,
        );
    }

    /// Jump the selection to the session with the given 1-based number,
    /// update the selection state, and refresh the preview pane.
    /// Does nothing if the number is out of range.
    /// Numbering matches `TreeList::to_list_items` — the Nth `Worktree` variant.
    fn jump_to_session_number(&mut self, number: usize) {
        if let Some(idx) = session_number_to_list_index(&self.ui_state.list_items, number) {
            self.ui_state.list_state.list_state.select(Some(idx));
            self.update_selection();
            self.ui_state.preview_update_spawned_at = None;
            self.spawn_preview_update();
        }
    }

    /// Check if a project (not a session) is currently selected
    fn is_project_selected(&self) -> bool {
        self.ui_state.selected_session_id.is_none() && self.ui_state.selected_project_id.is_some()
    }

    /// Render the preview pane
    fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        let is_focused = matches!(self.ui_state.focused_pane, FocusedPane::RightPane);
        let dim_opacity = if !is_focused && self.config.dim_unfocused_preview {
            Some(self.config.dim_unfocused_opacity)
        } else {
            None
        };

        // Show tab indicator in title
        let title = " [Preview] | Info | Shell ";

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if is_focused {
                self.theme.border_focused()
            } else {
                self.theme.border_unfocused()
            });

        // Update preview state with visible area
        let inner_height = area.height.saturating_sub(2);
        self.ui_state
            .preview_state
            .set_content(&self.ui_state.preview_content, inner_height);

        let preview = Preview::new(&self.ui_state.preview_content)
            .block(block)
            .scroll(self.ui_state.preview_state.scroll_offset)
            .dim_opacity(dim_opacity);

        frame.render_widget(preview, area);
    }

    /// Render the info pane (session metadata, PR details, AI summary)
    fn render_info(&mut self, frame: &mut Frame, area: Rect) {
        let is_focused = matches!(self.ui_state.focused_pane, FocusedPane::RightPane);
        let dim_opacity = if !is_focused && self.config.dim_unfocused_preview {
            Some(self.config.dim_unfocused_opacity)
        } else {
            None
        };
        let on_project = self.is_project_selected();

        // Compute display string for the generate-summary hotkey (None = AI disabled)
        let summary_key_hint = if self.config.ai_summary_enabled {
            self.config
                .keybindings
                .keys_for(BindableAction::GenerateSummary)
                .first()
                .map(|k| k.to_string())
        } else {
            None
        };

        let title = if on_project {
            " Shell | [Info] "
        } else {
            " Preview | [Info] | Shell "
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if is_focused {
                self.theme.border_focused()
            } else {
                self.theme.border_unfocused()
            });

        // Build the info content based on current selection
        let content = if let Some(session_id) = self.ui_state.selected_session_id {
            // Find the session data from list_items (includes all needed fields)
            let session_data = self.ui_state.list_items.iter().find_map(|item| {
                if let SessionListItem::Worktree {
                    id,
                    title,
                    branch,
                    status,
                    program,
                    pr_number,
                    pr_url,
                    pr_merged,
                    worktree_path,
                    created_at,
                    ..
                } = item
                {
                    if *id == session_id {
                        Some((
                            title.clone(),
                            branch.clone(),
                            *status,
                            program.clone(),
                            *pr_number,
                            pr_url.clone(),
                            *pr_merged,
                            worktree_path.display().to_string(),
                            created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                }
            });

            if let Some((
                title,
                branch,
                status,
                program,
                pr_number,
                pr_url,
                pr_merged,
                worktree_path,
                created_at,
            )) = session_data
            {
                let enriched_pr = self
                    .ui_state
                    .enriched_pr
                    .as_ref()
                    .and_then(|(sid, pr)| if *sid == session_id { Some(pr) } else { None });

                let ai_summary = if self.config.ai_summary_enabled {
                    self.ui_state.ai_summaries.get(&session_id)
                } else {
                    None
                };

                let data = InfoSessionData {
                    title,
                    branch,
                    created_at,
                    status,
                    program,
                    worktree_path,
                    diff_info: &self.ui_state.diff_info,
                    pr_number,
                    pr_url,
                    pr_merged,
                    enriched_pr,
                    ai_summary,
                    summary_key_hint: summary_key_hint.clone(),
                };

                // Count lines for scroll state
                let line_count = InfoView::new(InfoContent::Session(data), &self.theme)
                    .build_lines()
                    .len();
                let inner_height = area.height.saturating_sub(2);
                self.ui_state
                    .info_state
                    .set_metrics(line_count, inner_height);

                // Rebuild data (it was consumed by the line count call)
                let enriched_pr = self
                    .ui_state
                    .enriched_pr
                    .as_ref()
                    .and_then(|(sid, pr)| if *sid == session_id { Some(pr) } else { None });
                let ai_summary = if self.config.ai_summary_enabled {
                    self.ui_state.ai_summaries.get(&session_id)
                } else {
                    None
                };
                // Re-find session data (original was consumed)
                let session_data2 = self.ui_state.list_items.iter().find_map(|item| {
                    if let SessionListItem::Worktree {
                        id,
                        title,
                        branch,
                        status,
                        program,
                        pr_number,
                        pr_url,
                        pr_merged,
                        worktree_path,
                        created_at,
                        ..
                    } = item
                    {
                        if *id == session_id {
                            Some((
                                title.clone(),
                                branch.clone(),
                                *status,
                                program.clone(),
                                *pr_number,
                                pr_url.clone(),
                                *pr_merged,
                                worktree_path.display().to_string(),
                                created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                if let Some((t, b, s, p, pn, pu, pm, wp, ca)) = session_data2 {
                    InfoContent::Session(InfoSessionData {
                        title: t,
                        branch: b,
                        created_at: ca,
                        status: s,
                        program: p,
                        worktree_path: wp,
                        diff_info: &self.ui_state.diff_info,
                        pr_number: pn,
                        pr_url: pu,
                        pr_merged: pm,
                        enriched_pr,
                        ai_summary,
                        summary_key_hint,
                    })
                } else {
                    InfoContent::Empty
                }
            } else {
                InfoContent::Empty
            }
        } else if let Some(project_id) = self.ui_state.selected_project_id {
            let project_data = self.ui_state.list_items.iter().find_map(|item| {
                if let SessionListItem::Project {
                    id,
                    name,
                    repo_path,
                    main_branch,
                    ..
                } = item
                {
                    if *id == project_id {
                        Some((
                            name.clone(),
                            repo_path.display().to_string(),
                            main_branch.clone(),
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                }
            });

            if let Some((name, repo_path, main_branch)) = project_data {
                let inner_height = area.height.saturating_sub(2);
                self.ui_state.info_state.set_metrics(3, inner_height);

                InfoContent::Project(InfoProjectData {
                    name,
                    repo_path,
                    main_branch,
                })
            } else {
                InfoContent::Empty
            }
        } else {
            InfoContent::Empty
        };

        let info_view = InfoView::new(content, &self.theme)
            .block(block)
            .scroll(self.ui_state.info_state.scroll_offset)
            .dim_opacity(dim_opacity);

        frame.render_widget(info_view, area);
    }

    /// Render the shell pane
    fn render_shell(&mut self, frame: &mut Frame, area: Rect) {
        let is_focused = matches!(self.ui_state.focused_pane, FocusedPane::RightPane);
        let dim_opacity = if !is_focused && self.config.dim_unfocused_preview {
            Some(self.config.dim_unfocused_opacity)
        } else {
            None
        };

        let title = if self.is_project_selected() {
            " [Shell] | Info "
        } else {
            " Preview | Info | [Shell] "
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if is_focused {
                self.theme.border_focused()
            } else {
                self.theme.border_unfocused()
            });

        let inner_height = area.height.saturating_sub(2);
        self.ui_state
            .shell_state
            .set_content(&self.ui_state.shell_content, inner_height);

        let preview = Preview::new(&self.ui_state.shell_content)
            .block(block)
            .scroll(self.ui_state.shell_state.scroll_offset)
            .dim_opacity(dim_opacity);

        frame.render_widget(preview, area);
    }

    /// Render modal overlay
    fn render_modal(&mut self, frame: &mut Frame, area: Rect) {
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
                    .border_style(Style::default().fg(self.theme.modal_warning));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\n> {}_", prompt, value);
                let paragraph = Paragraph::new(text);
                frame.render_widget(paragraph, inner);
            }

            Modal::PathInput {
                title,
                prompt,
                value,
                completer,
                ..
            } => {
                let modal_area = centered_rect(60, 40, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_warning));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                // Split: prompt+input at top, completions below, hint at bottom
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // prompt + input
                        Constraint::Min(1),    // completions list
                        Constraint::Length(1), // hint line
                    ])
                    .split(inner);

                let input_text = format!("{}\n\n> {}_", prompt, value);
                let input_para = Paragraph::new(input_text);
                frame.render_widget(input_para, chunks[0]);

                // Render completions list
                let (completions, highlighted) = completer.visible_completions();
                if !completions.is_empty() {
                    let lines: Vec<Line> = completions
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            // Show just the final path component for readability
                            let display = c.rsplit('/').next().unwrap_or(c);
                            if highlighted == Some(i) {
                                Line::from(Span::styled(
                                    format!("  > {}", display),
                                    Style::default()
                                        .fg(self.theme.modal_info)
                                        .add_modifier(Modifier::BOLD),
                                ))
                            } else {
                                Line::from(format!("    {}", display))
                            }
                        })
                        .collect();
                    let completions_para = Paragraph::new(lines);
                    frame.render_widget(completions_para, chunks[1]);
                }

                let hint = Line::from(Span::styled(
                    "[Tab] complete  [Enter] submit  [Esc] cancel",
                    Style::default().add_modifier(Modifier::DIM),
                ));
                frame.render_widget(Paragraph::new(hint), chunks[2]);
            }

            Modal::Loading { title, message } => {
                let modal_area = centered_rect(60, 20, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                const RAINBOW: &[ratatui::style::Color] = &[
                    ratatui::style::Color::Red,
                    ratatui::style::Color::Yellow,
                    ratatui::style::Color::Green,
                    ratatui::style::Color::Cyan,
                    ratatui::style::Color::Blue,
                    ratatui::style::Color::Magenta,
                ];
                let color = RAINBOW[self.ui_state.throbber_state.index() as usize % RAINBOW.len()];
                let throbber = throbber_widgets_tui::Throbber::default()
                    .throbber_set(throbber_widgets_tui::symbols::throbber::BRAILLE_EIGHT)
                    .label(message.as_str())
                    .throbber_style(Style::default().fg(color));
                frame.render_stateful_widget(throbber, inner, &mut self.ui_state.throbber_state);
            }

            Modal::Confirm { title, message, .. } => {
                let modal_area = centered_rect(50, 15, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(format!(" {} ", title))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_error));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\n[Enter] Confirm  [Esc] Cancel", message);
                let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
                frame.render_widget(paragraph, inner);
            }

            Modal::Error { message } => {
                let modal_area = centered_rect(60, 20, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(" Error ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_error));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                let text = format!("{}\n\nPress any key to close.", message);
                let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
                frame.render_widget(paragraph, inner);
            }

            Modal::Help => {
                let modal_area = centered_rect(70, 80, area);
                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(" Help ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                // Add margin inside the modal for better readability
                let content_area = inner.inner(Margin {
                    horizontal: 2,
                    vertical: 1,
                });

                let help_lines = self.build_help_lines();

                let paragraph = Paragraph::new(help_lines);
                frame.render_widget(paragraph, content_area);
            }

            Modal::Settings(state) => {
                self.render_settings_modal(frame, area, state);
            }

            Modal::QuickSwitch {
                query,
                matches,
                selected_idx,
            } => {
                let max_visible = 10;
                let visible_matches = matches.len().min(max_visible);
                // Dynamic height: border(2) + input(1) + matches
                let modal_height = (3 + visible_matches) as u16;
                let modal_width = (area.width * 60 / 100).max(40);

                // Position in upper third
                let modal_area = Rect {
                    x: area.x + (area.width.saturating_sub(modal_width)) / 2,
                    y: area.y + area.height / 5,
                    width: modal_width,
                    height: modal_height.min(area.height),
                };

                frame.render_widget(Clear, modal_area);

                let block = Block::default()
                    .title(" Quick Switch ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.modal_info));

                let inner = block.inner(modal_area);
                frame.render_widget(block, modal_area);

                if inner.height == 0 {
                    return;
                }

                // Input line
                let input_line = Line::from(format!("> {}_", query));
                let input_area = Rect { height: 1, ..inner };
                frame.render_widget(Paragraph::new(input_line), input_area);

                // Match lines
                for (i, m) in matches.iter().take(max_visible).enumerate() {
                    let row = inner.y + 1 + i as u16;
                    if row >= inner.y + inner.height {
                        break;
                    }

                    let status_icon = match m.status {
                        SessionStatus::Creating => "⠋",
                        SessionStatus::Running => "●",
                        SessionStatus::Stopped => "○",
                    };
                    let status_color = match m.status {
                        SessionStatus::Creating => self.theme.status_creating,
                        SessionStatus::Running => self.theme.status_running,
                        SessionStatus::Stopped => self.theme.status_stopped,
                    };

                    let is_selected = i == *selected_idx;
                    let mut spans = vec![
                        Span::styled(
                            format!(" {} ", status_icon),
                            Style::default().fg(status_color),
                        ),
                        Span::styled(
                            m.title.clone(),
                            if is_selected {
                                self.theme.selection()
                            } else {
                                Style::default()
                            },
                        ),
                    ];
                    if let Some(shown_branch) = crate::session::display_branch(&m.title, &m.branch)
                    {
                        spans.push(Span::styled(
                            format!(" [{}]", shown_branch),
                            Style::default().fg(self.theme.text_accent),
                        ));
                    }
                    spans.push(Span::styled(
                        format!(" ({})", m.project_name),
                        Style::default().fg(self.theme.text_secondary),
                    ));
                    let line = Line::from(spans);

                    let line_area = Rect {
                        y: row,
                        height: 1,
                        ..inner
                    };
                    frame.render_widget(Paragraph::new(line), line_area);
                }
            }
        }
    }

    /// Build rows for the settings modal for the given tab.
    fn build_settings_rows(&self, tab: SettingsTab) -> Vec<SettingsRow> {
        match tab {
            SettingsTab::General => {
                let c = &self.config;
                vec![
                    SettingsRow {
                        label: "Default Program".into(),
                        value: c.default_program.clone(),
                        field_key: "default_program".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Branch Prefix".into(),
                        value: if c.branch_prefix.is_empty() {
                            "(none)".into()
                        } else {
                            c.branch_prefix.clone()
                        },
                        field_key: "branch_prefix".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Shell Program".into(),
                        value: c.shell_program.clone(),
                        field_key: "shell_program".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Editor".into(),
                        value: c.editor.clone().unwrap_or_else(|| "(auto)".into()),
                        field_key: "editor".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Editor is GUI".into(),
                        value: match c.editor_gui {
                            Some(true) => "true".into(),
                            Some(false) => "false".into(),
                            None => "(auto)".into(),
                        },
                        field_key: "editor_gui".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Fetch Before Create".into(),
                        value: c.fetch_before_create.to_string(),
                        field_key: "fetch_before_create".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Resume Session".into(),
                        value: c.resume_session.to_string(),
                        field_key: "resume_session".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "UI Refresh FPS".into(),
                        value: c.ui_refresh_fps.to_string(),
                        field_key: "ui_refresh_fps".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "PR Check Interval (s)".into(),
                        value: c.pr_check_interval_secs.to_string(),
                        field_key: "pr_check_interval_secs".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Max Concurrent Tmux".into(),
                        value: c.max_concurrent_tmux.to_string(),
                        field_key: "max_concurrent_tmux".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Dim Unfocused Preview".into(),
                        value: c.dim_unfocused_preview.to_string(),
                        field_key: "dim_unfocused_preview".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Dim Opacity".into(),
                        value: format!("{:.2}", c.dim_unfocused_opacity),
                        field_key: "dim_unfocused_opacity".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Session Numbers".into(),
                        value: c.show_session_numbers.to_string(),
                        field_key: "show_session_numbers".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Invert PR Label Color".into(),
                        value: c.invert_pr_label_color.to_string(),
                        field_key: "invert_pr_label_color".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "Number Debounce (ms)".into(),
                        value: c.session_number_debounce_ms.to_string(),
                        field_key: "session_number_debounce_ms".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "AI Summary Enabled".into(),
                        value: c.ai_summary_enabled.to_string(),
                        field_key: "ai_summary_enabled".into(),
                        color_swatch: None,
                    },
                    SettingsRow {
                        label: "AI Summary Model".into(),
                        value: c.ai_summary_model.clone(),
                        field_key: "ai_summary_model".into(),
                        color_swatch: None,
                    },
                ]
            }
            SettingsTab::Keybindings => {
                let kb = &self.config.keybindings;
                BindableAction::ALL
                    .iter()
                    .map(|&action| SettingsRow {
                        label: action.description().to_string(),
                        value: kb.keys_display(action),
                        field_key: action.config_name().to_string(),
                        color_swatch: None,
                    })
                    .collect()
            }
            SettingsTab::Theme => {
                // Show the current resolved color for each overridable field,
                // and whether it has a user override.
                let t = &self.theme;
                let o = &self.config.theme;

                macro_rules! theme_row {
                    ($label:expr, $field:ident) => {
                        SettingsRow {
                            label: $label.into(),
                            value: o
                                .$field
                                .map(|cv| {
                                    let s = toml::to_string(&cv).unwrap_or_default();
                                    s.trim().trim_matches('"').to_string()
                                })
                                .unwrap_or_else(|| format_color(t.$field)),
                            field_key: stringify!($field).into(),
                            color_swatch: Some(t.$field),
                        }
                    };
                }

                vec![
                    SettingsRow {
                        label: "Preset".into(),
                        value: o.preset.clone().unwrap_or_else(|| "(auto)".into()),
                        field_key: "preset".into(),
                        color_swatch: None,
                    },
                    theme_row!("Border Focused", border_focused),
                    theme_row!("Border Unfocused", border_unfocused),
                    theme_row!("Selection BG", selection_bg),
                    theme_row!("Status Running", status_running),
                    theme_row!("Status Stopped", status_stopped),
                    theme_row!("Status PR", status_pr),
                    theme_row!("Status PR Merged", status_pr_merged),
                    theme_row!("PR Open", pr_open),
                    theme_row!("PR Draft", pr_draft),
                    theme_row!("PR Closed", pr_closed),
                    theme_row!("Text Primary", text_primary),
                    theme_row!("Text Secondary", text_secondary),
                    theme_row!("Text Accent", text_accent),
                    theme_row!("Diff Added", diff_added),
                    theme_row!("Diff Removed", diff_removed),
                    theme_row!("Diff Hunk Header", diff_hunk_header),
                    theme_row!("Diff File Header", diff_file_header),
                    theme_row!("Modal Info", modal_info),
                    theme_row!("Modal Warning", modal_warning),
                    theme_row!("Modal Error", modal_error),
                    theme_row!("Status Bar BG", status_bar_bg),
                    theme_row!("Status Bar FG", status_bar_fg),
                ]
            }
        }
    }

    /// Render the settings modal.
    fn render_settings_modal(&self, frame: &mut Frame, area: Rect, state: &SettingsState) {
        let modal_area = centered_rect(75, 85, area);
        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Settings ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.modal_info));
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let content_area = inner.inner(Margin {
            horizontal: 1,
            vertical: 0,
        });

        if content_area.height < 4 {
            return;
        }

        // --- Tab bar (row 0) ---
        let tab_area = Rect {
            height: 1,
            ..content_area
        };
        let mut tab_spans: Vec<Span> = Vec::new();
        for (i, tab) in SettingsTab::ALL.iter().enumerate() {
            if i > 0 {
                tab_spans.push(Span::raw("  "));
            }
            let style = if *tab == state.tab {
                Style::default()
                    .fg(self.theme.text_primary)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(self.theme.text_secondary)
            };
            tab_spans.push(Span::styled(tab.label(), style));
        }
        frame.render_widget(Paragraph::new(Line::from(tab_spans)), tab_area);

        // --- Separator ---
        let sep_area = Rect {
            y: content_area.y + 1,
            height: 1,
            ..content_area
        };
        let separator = "─".repeat(content_area.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                separator,
                Style::default().fg(self.theme.border_unfocused),
            ))),
            sep_area,
        );

        // --- Settings rows ---
        let rows_area = Rect {
            y: content_area.y + 2,
            height: content_area.height.saturating_sub(4),
            ..content_area
        };

        let label_width = 24_u16;
        let value_width = rows_area.width.saturating_sub(label_width + 3);

        let visible_rows = rows_area.height as usize;
        let scroll_offset = if state.selected_row >= visible_rows {
            state.selected_row - visible_rows + 1
        } else {
            0
        };

        for (i, row) in state
            .rows
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_rows)
        {
            let y = rows_area.y + (i - scroll_offset) as u16;
            let is_selected = i == state.selected_row;

            let row_style = if is_selected {
                self.theme.selection()
            } else {
                Style::default()
            };

            // Label
            let label_area = Rect {
                x: rows_area.x,
                y,
                width: label_width.min(rows_area.width),
                height: 1,
            };
            let label = format!("{:<width$}", row.label, width = label_width as usize);
            frame.render_widget(Paragraph::new(Span::styled(label, row_style)), label_area);

            // Color swatch + Value
            if rows_area.width > label_width + 2 {
                let swatch_width: u16 = if row.color_swatch.is_some() { 3 } else { 0 };
                let val_x = rows_area.x + label_width + 2;

                // Render color swatch if present
                if let Some(swatch_color) = row.color_swatch {
                    let swatch_area = Rect {
                        x: val_x,
                        y,
                        width: swatch_width.min(value_width),
                        height: 1,
                    };
                    let swatch_style = if is_selected {
                        Style::default()
                            .fg(swatch_color)
                            .bg(self.theme.selection_bg)
                    } else {
                        Style::default().fg(swatch_color)
                    };
                    frame.render_widget(
                        Paragraph::new(Span::styled("██ ", swatch_style)),
                        swatch_area,
                    );
                }

                let val_area = Rect {
                    x: val_x + swatch_width,
                    y,
                    width: value_width.saturating_sub(swatch_width),
                    height: 1,
                };

                let display_val = if is_selected {
                    if let Some(SettingsEditing::TextInput { value }) = &state.editing {
                        format!("{value}▏")
                    } else {
                        row.value.clone()
                    }
                } else {
                    row.value.clone()
                };

                let val_style = if is_selected && state.editing.is_some() {
                    row_style.add_modifier(Modifier::UNDERLINED)
                } else {
                    row_style.fg(self.theme.text_accent)
                };

                frame.render_widget(
                    Paragraph::new(Span::styled(display_val, val_style)),
                    val_area,
                );
            }
        }

        // --- Footer ---
        let footer_area = Rect {
            y: content_area.y + content_area.height.saturating_sub(1),
            height: 1,
            ..content_area
        };
        let footer_text = if state.editing.is_some() {
            "Enter: save  Esc: cancel"
        } else {
            "Tab: switch tab  j/k: navigate  Enter: edit  Esc: close"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                footer_text,
                Style::default().fg(self.theme.text_secondary),
            )),
            footer_area,
        );
    }

    /// Apply an edited value from the settings modal to the config.
    fn apply_settings_edit(&mut self, tab: SettingsTab, field_key: &str, value: &str) {
        match tab {
            SettingsTab::General => match field_key {
                "default_program" => self.config.default_program = value.to_string(),
                "branch_prefix" => self.config.branch_prefix = value.to_string(),
                "shell_program" => self.config.shell_program = value.to_string(),
                "editor" => {
                    self.config.editor = if value.is_empty() || value == "(auto)" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                }
                "editor_gui" => {
                    self.config.editor_gui = match value {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    };
                }
                "fetch_before_create" => {
                    if let Ok(b) = value.parse::<bool>() {
                        self.config.fetch_before_create = b;
                    }
                }
                "resume_session" => {
                    if let Ok(b) = value.parse::<bool>() {
                        self.config.resume_session = b;
                    }
                }
                "ui_refresh_fps" => {
                    if let Ok(v) = value.parse::<u32>() {
                        self.config.ui_refresh_fps = v;
                    }
                }
                "pr_check_interval_secs" => {
                    if let Ok(v) = value.parse::<u64>() {
                        self.config.pr_check_interval_secs = v;
                    }
                }
                "max_concurrent_tmux" => {
                    if let Ok(v) = value.parse::<usize>() {
                        self.config.max_concurrent_tmux = v;
                    }
                }
                "dim_unfocused_preview" => {
                    if let Ok(b) = value.parse::<bool>() {
                        self.config.dim_unfocused_preview = b;
                    }
                }
                "dim_unfocused_opacity" => {
                    if let Ok(v) = value.parse::<f32>() {
                        self.config.dim_unfocused_opacity = v.clamp(0.0, 1.0);
                    }
                }
                "show_session_numbers" => {
                    if let Ok(b) = value.parse::<bool>() {
                        self.config.show_session_numbers = b;
                    }
                }
                "invert_pr_label_color" => {
                    if let Ok(b) = value.parse::<bool>() {
                        self.config.invert_pr_label_color = b;
                    }
                }
                "session_number_debounce_ms" => {
                    if let Ok(v) = value.parse::<u64>() {
                        self.config.session_number_debounce_ms = v;
                    }
                }
                "ai_summary_enabled" => {
                    if let Ok(b) = value.parse::<bool>() {
                        self.config.ai_summary_enabled = b;
                    }
                }
                "ai_summary_model" => {
                    self.config.ai_summary_model = value.to_string();
                }
                _ => {}
            },
            SettingsTab::Theme => {
                use crate::config::theme::ColorValue;

                if field_key == "preset" {
                    self.config.theme.preset = if value.is_empty() || value == "(auto)" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                } else {
                    // Try to parse the value as a ColorValue via TOML
                    let toml_input = if value.starts_with('#')
                        || value.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
                    {
                        format!("c = \"{value}\"")
                    } else {
                        format!("c = {value}")
                    };

                    #[derive(serde::Deserialize)]
                    struct Wrap {
                        c: ColorValue,
                    }

                    if let Ok(w) = toml::from_str::<Wrap>(&toml_input) {
                        macro_rules! set_theme_field {
                            ($($name:ident),*) => {
                                match field_key {
                                    $(stringify!($name) => self.config.theme.$name = Some(w.c),)*
                                    _ => {}
                                }
                            };
                        }
                        set_theme_field!(
                            border_focused,
                            border_unfocused,
                            selection_bg,
                            selection_fg,
                            status_running,
                            status_stopped,
                            status_pr,
                            status_pr_merged,
                            pr_open,
                            pr_draft,
                            pr_closed,
                            text_primary,
                            text_secondary,
                            text_accent,
                            diff_added,
                            diff_removed,
                            diff_hunk_header,
                            diff_file_header,
                            diff_context,
                            modal_info,
                            modal_warning,
                            modal_error,
                            status_bar_bg,
                            status_bar_fg
                        );
                    }
                }

                // Rebuild theme from updated overrides
                let base = self
                    .config
                    .theme
                    .preset
                    .as_deref()
                    .and_then(Theme::from_preset)
                    .unwrap_or_default();
                self.theme = base.with_overrides(&self.config.theme);
            }
            SettingsTab::Keybindings => {
                use crate::config::keybindings::{BindableAction, KeyBinding};
                use std::str::FromStr;

                let Ok(action) = BindableAction::from_str(field_key) else {
                    warn!("Unknown keybinding action: {}", field_key);
                    return;
                };

                // The row value is rendered as a comma-separated list
                // (e.g. `"k, Up, Ctrl-p"`). Parse each entry back into a
                // `KeyBinding`, ignoring empty tokens. If every token fails
                // to parse we leave the binding alone rather than silently
                // clear it.
                let mut parsed: Vec<KeyBinding> = Vec::new();
                let mut had_token = false;
                let mut any_err = false;
                for token in value.split(',') {
                    let t = token.trim();
                    if t.is_empty() {
                        continue;
                    }
                    had_token = true;
                    match KeyBinding::from_str(t) {
                        Ok(kb) => parsed.push(kb),
                        Err(e) => {
                            warn!("Invalid keybinding '{}': {}", t, e);
                            any_err = true;
                        }
                    }
                }

                if had_token && parsed.is_empty() && any_err {
                    // User tried to edit but every token was malformed —
                    // show the error but don't wipe their existing binding.
                    self.ui_state.modal = Modal::Error {
                        message: format!(
                            "Could not parse any key bindings from '{}'. \
                             Use e.g. 'k', 'Ctrl-p', 'Shift-N', 'Enter'.",
                            value
                        ),
                    };
                    return;
                }

                self.config.keybindings.set_keys_for(action, parsed);
            }
        }

        // Persist config via the store (updates mtime so hot-reload won't re-read our own write)
        let updated = self.config.clone();
        if let Err(e) = self.config_store.mutate(|c| *c = updated) {
            warn!("Failed to save config: {}", e);
        }
    }

    /// Handle a keypress in the settings modal.
    async fn handle_settings_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        mut state: SettingsState,
    ) {
        use crossterm::event::KeyCode;

        if let Some(ref mut editing) = state.editing {
            // Currently editing a field
            match editing {
                SettingsEditing::TextInput { value } => match key.code {
                    KeyCode::Enter => {
                        let val = value.clone();
                        let field_key = state.rows[state.selected_row].field_key.clone();
                        state.editing = None;
                        self.apply_settings_edit(state.tab, &field_key, &val);
                        // Refresh rows after applying
                        state.rows = self.build_settings_rows(state.tab);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Esc => {
                        state.editing = None;
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Backspace => {
                        value.pop();
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Char(c) => {
                        value.push(c);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    _ => {
                        self.ui_state.modal = Modal::Settings(state);
                    }
                },
                SettingsEditing::KeyCapture { .. } => {
                    // For key capture, any keypress except Esc is captured as the new binding
                    match key.code {
                        KeyCode::Esc => {
                            state.editing = None;
                            self.ui_state.modal = Modal::Settings(state);
                        }
                        _ => {
                            // Key capture is a simplified version — store the key display
                            // Full keybinding editing would require more complex UX
                            state.editing = None;
                            self.ui_state.modal = Modal::Settings(state);
                        }
                    }
                }
            }
        } else {
            // Not editing — navigation mode: resolve via configurable keybindings
            use crate::config::keybindings::BindableAction;

            match self.config.keybindings.resolve(&key) {
                Some(BindableAction::NavigateDown) => {
                    if !state.rows.is_empty() {
                        state.selected_row = (state.selected_row + 1) % state.rows.len();
                    }
                    self.ui_state.modal = Modal::Settings(state);
                }
                Some(BindableAction::NavigateUp) => {
                    if !state.rows.is_empty() {
                        state.selected_row = if state.selected_row == 0 {
                            state.rows.len() - 1
                        } else {
                            state.selected_row - 1
                        };
                    }
                    self.ui_state.modal = Modal::Settings(state);
                }
                Some(BindableAction::Quit) => {
                    self.ui_state.modal = Modal::None;
                }
                _ => match key.code {
                    KeyCode::Esc => {
                        self.ui_state.modal = Modal::None;
                    }
                    KeyCode::Tab => {
                        state.tab = state.tab.next();
                        state.selected_row = 0;
                        state.rows = self.build_settings_rows(state.tab);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::BackTab => {
                        state.tab = state.tab.prev();
                        state.selected_row = 0;
                        state.rows = self.build_settings_rows(state.tab);
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    KeyCode::Enter => {
                        if !state.rows.is_empty() {
                            let current_value = state.rows[state.selected_row].value.clone();
                            let initial = if current_value == "(auto)" || current_value == "(none)"
                            {
                                String::new()
                            } else {
                                current_value
                            };
                            state.editing = Some(SettingsEditing::TextInput { value: initial });
                        }
                        self.ui_state.modal = Modal::Settings(state);
                    }
                    _ => {
                        self.ui_state.modal = Modal::Settings(state);
                    }
                },
            }
        }
    }

    /// Build help screen lines dynamically from the keybinding table.
    fn build_help_lines(&self) -> Vec<Line<'static>> {
        let kb = &self.config.keybindings;
        let mut lines: Vec<Line<'static>> = Vec::new();
        let key_col_width = 18;

        for (section_name, actions) in kb.sections() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(format!("{section_name}:")));

            for (action, keys_str) in &actions {
                let desc = action.description();
                let padded_keys = format!("  {keys_str:<width$}{desc}", width = key_col_width);
                lines.push(Line::from(padded_keys));
            }
        }

        // Quick-switch (hardcoded since leader_key is in config, not keybindings)
        lines.push(Line::from(""));
        lines.push(Line::from("Quick Switch:"));
        let leader_display =
            if self.config.leader_key.trim().is_empty() || self.config.leader_key == " " {
                "Space".to_string()
            } else {
                self.config.leader_key.clone()
            };
        lines.push(Line::from(format!(
            "  {:<width$}Fuzzy session search",
            leader_display,
            width = key_col_width,
        )));

        // Status indicators (not keybinding-related, stays hardcoded)
        lines.push(Line::from(""));
        lines.push(Line::from("Status Indicators:"));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.status_running)),
            Span::raw("  Running (agent active)"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.status_pr)),
            Span::raw("  PR open"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("●", Style::default().fg(self.theme.status_pr_merged)),
            Span::raw("  PR merged"),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("○", Style::default().fg(self.theme.status_stopped)),
            Span::raw("  Stopped"),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from("Press any key to close this help."));

        lines
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

        let status = if let Some((ref msg, expires)) = self.ui_state.status_message {
            if Instant::now() < expires {
                msg.clone()
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let restart_needed = self.config_store.restart_required();

        let status = if status.is_empty() {
            let session_count = self
                .ui_state
                .list_items
                .iter()
                .filter(|i| i.is_worktree())
                .count();
            if restart_needed {
                format!(
                    "Sessions: {} | Restart to apply config changes | ? help",
                    session_count
                )
            } else {
                format!(
                    "Sessions: {} | Press ? for help | n: new session | N: add project",
                    session_count
                )
            }
        } else if restart_needed {
            format!("{} | Restart to apply config changes", status)
        } else {
            status
        };

        let paragraph = Paragraph::new(status).style(self.theme.status_bar());

        frame.render_widget(paragraph, status_area);
    }

    /// Handle input events
    async fn handle_input(&mut self, input: InputEvent) {
        match input {
            InputEvent::Key(key) => {
                debug!(
                    "Key event: code={:?} modifiers={:?} kind={:?}",
                    key.code, key.modifiers, key.kind
                );

                // Suppress stray bytes from unrecognized escape sequences.
                // When crossterm can't parse a multi-byte sequence (e.g. from
                // modifier combos the terminal encodes as CSI), it emits each
                // byte as a separate key event ~8ms apart.  We suppress all
                // events for a short window after an unrecognized one.
                let now = Instant::now();
                if now < self.suppress_keys_until {
                    debug!("Suppressing key event (escape sequence cooldown)");
                    return;
                }

                // Check for modal-specific handling first
                if !matches!(self.ui_state.modal, Modal::None) {
                    self.handle_modal_key(key).await;
                    return;
                }

                // Check for configurable leader key (quick-switch)
                let (leader_code, leader_mods) = self.config.parse_leader_key();
                if key.code == leader_code && key.modifiers == leader_mods {
                    self.open_quick_switch().await;
                    return;
                }

                // Number-jump: intercept digit keys when session numbers are enabled
                if self.config.show_session_numbers
                    && let crossterm::event::KeyCode::Char(c @ '0'..='9') = key.code
                    && key.modifiers.is_empty()
                {
                    let digit = c as u8 - b'0';
                    if let super::digit_accumulator::DigitResult::Jump(n) =
                        self.digit_accumulator.press(digit)
                    {
                        self.jump_to_session_number(n);
                    }
                    return;
                }

                // Convert to command and handle
                match UserCommand::from_key(key, &self.config.keybindings) {
                    Some(cmd) => self.handle_command(cmd).await,
                    None => {
                        // Unrecognized key event — likely the start of a
                        // broken escape sequence.  Suppress further events
                        // briefly so trailing bytes don't trigger commands.
                        self.suppress_keys_until = now + Duration::from_millis(50);
                    }
                }
            }
            InputEvent::Resize(_, _) => {
                // Terminal will re-render automatically
            }
            InputEvent::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_pane_at(mouse.column, ScrollDirection::Up);
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_pane_at(mouse.column, ScrollDirection::Down);
                }
                _ => {}
            },
            InputEvent::Paste(text) => {
                // Handle paste in modal input, ignore otherwise
                let clean = text.replace(['\n', '\r'], "");
                match &mut self.ui_state.modal {
                    Modal::Input { value, .. } => {
                        value.push_str(&clean);
                    }
                    Modal::PathInput {
                        value, completer, ..
                    } => {
                        value.push_str(&clean);
                        completer.invalidate();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Handle modal key input
    async fn handle_modal_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        match &mut self.ui_state.modal {
            Modal::Input {
                value, on_submit, ..
            } => match key.code {
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
            },

            Modal::PathInput {
                value,
                on_submit,
                completer,
                ..
            } => match key.code {
                KeyCode::Enter => {
                    let action = on_submit.clone();
                    let value = value.clone();
                    self.ui_state.modal = Modal::None;
                    self.handle_input_submit(action, value).await;
                }
                KeyCode::Esc => {
                    self.ui_state.modal = Modal::None;
                }
                KeyCode::Tab => {
                    let completed = completer.complete(value);
                    *value = completed;
                }
                KeyCode::Backspace => {
                    value.pop();
                    completer.invalidate();
                }
                KeyCode::Char(c) => {
                    value.push(c);
                    completer.invalidate();
                }
                _ => {}
            },

            Modal::Confirm { on_confirm, .. } => match key.code {
                KeyCode::Enter => {
                    let action = on_confirm.clone();
                    self.ui_state.modal = Modal::None;
                    self.handle_confirm(action).await;
                }
                KeyCode::Esc => {
                    self.ui_state.modal = Modal::None;
                }
                _ => {}
            },

            Modal::Loading { .. } => {
                // Non-interactive — swallow all keys while loading
            }

            Modal::Help | Modal::Error { .. } => {
                // Any key closes help/error
                self.ui_state.modal = Modal::None;
            }

            Modal::Settings(_) => {
                // Extract the state to avoid borrow conflict with &mut self
                let state = match std::mem::replace(&mut self.ui_state.modal, Modal::None) {
                    Modal::Settings(s) => s,
                    _ => unreachable!(),
                };
                self.handle_settings_key(key, state).await;
            }

            Modal::QuickSwitch {
                query,
                matches,
                selected_idx,
            } => {
                use crate::config::keybindings::BindableAction;

                // Resolve configurable bindings first for navigation
                match self.config.keybindings.resolve(&key) {
                    Some(BindableAction::NavigateUp) => {
                        if !matches.is_empty() {
                            *selected_idx = if *selected_idx == 0 {
                                matches.len() - 1
                            } else {
                                *selected_idx - 1
                            };
                        }
                    }
                    Some(BindableAction::NavigateDown) => {
                        if !matches.is_empty() {
                            *selected_idx = (*selected_idx + 1) % matches.len();
                        }
                    }
                    _ => match key.code {
                        KeyCode::Esc => {
                            self.ui_state.modal = Modal::None;
                        }
                        KeyCode::Enter => {
                            if let Some(m) = matches.get(*selected_idx) {
                                let session_id = m.session_id;
                                self.ui_state.modal = Modal::None;
                                self.ui_state.selected_session_id = Some(session_id);
                                if let Some(idx) =
                                    self.ui_state.list_items.iter().position(|item| {
                                        matches!(item, SessionListItem::Worktree { id, .. } if *id == session_id)
                                    })
                                {
                                    self.ui_state.list_state.select(Some(idx));
                                }
                                self.update_selection();
                                self.handle_select().await;
                            }
                        }
                        KeyCode::Tab => {
                            if let Some(m) = matches.get(*selected_idx) {
                                *query = m.title.clone();
                                self.refilter_quick_switch();
                            }
                        }
                        KeyCode::Backspace => {
                            query.pop();
                            self.refilter_quick_switch();
                        }
                        KeyCode::Char(c) => {
                            query.push(c);
                            self.refilter_quick_switch();
                        }
                        _ => {}
                    },
                }
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
            UserCommand::SelectShell => {
                self.handle_select_shell().await;
            }
            UserCommand::NewSession => {
                self.handle_new_session();
            }
            UserCommand::NewProject => {
                self.ui_state.modal = Modal::PathInput {
                    title: "Add Project".to_string(),
                    prompt: "Enter path to git repository:".to_string(),
                    value: std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    on_submit: InputAction::AddProject,
                    completer: PathCompleter::new(),
                };
            }
            UserCommand::DeleteSession => {
                self.handle_delete_session();
            }
            UserCommand::RestartSession => {
                self.handle_restart_session();
            }
            UserCommand::RemoveProject => {
                self.handle_remove_project();
            }
            UserCommand::OpenInEditor => {
                self.handle_open_in_editor().await;
            }
            UserCommand::OpenPullRequest => {
                self.handle_open_pull_request().await;
            }
            UserCommand::TogglePane => {
                let on_project = self.ui_state.selected_session_id.is_none()
                    && self.ui_state.selected_project_id.is_some();
                self.ui_state.right_pane_view = if on_project {
                    // Project: Shell → Info → Shell (no Preview)
                    match self.ui_state.right_pane_view {
                        RightPaneView::Shell => RightPaneView::Info,
                        _ => RightPaneView::Shell,
                    }
                } else {
                    // Session: Preview → Info → Shell → Preview
                    match self.ui_state.right_pane_view {
                        RightPaneView::Preview => RightPaneView::Info,
                        RightPaneView::Info => RightPaneView::Shell,
                        RightPaneView::Shell => RightPaneView::Preview,
                    }
                };
                self.ui_state.clear_right_pane = true;
                self.spawn_info_fetch();
            }
            UserCommand::TogglePaneReverse => {
                let on_project = self.ui_state.selected_session_id.is_none()
                    && self.ui_state.selected_project_id.is_some();
                self.ui_state.right_pane_view = if on_project {
                    // Project: Info → Shell → Info (no Preview)
                    match self.ui_state.right_pane_view {
                        RightPaneView::Info => RightPaneView::Shell,
                        _ => RightPaneView::Info,
                    }
                } else {
                    // Session: Shell → Info → Preview → Shell
                    match self.ui_state.right_pane_view {
                        RightPaneView::Preview => RightPaneView::Shell,
                        RightPaneView::Info => RightPaneView::Preview,
                        RightPaneView::Shell => RightPaneView::Info,
                    }
                };
                self.ui_state.clear_right_pane = true;
                self.spawn_info_fetch();
            }
            UserCommand::ShrinkLeftPane => {
                self.ui_state.left_pane_pct = self
                    .ui_state
                    .left_pane_pct
                    .saturating_sub(2)
                    .max(MIN_LEFT_PANE_PCT);
                self.save_left_pane_pct().await;
            }
            UserCommand::GrowLeftPane => {
                self.ui_state.left_pane_pct =
                    (self.ui_state.left_pane_pct + 2).min(MAX_LEFT_PANE_PCT);
                self.save_left_pane_pct().await;
            }
            UserCommand::ShowHelp => {
                self.ui_state.modal = Modal::Help;
            }
            UserCommand::ShowSettings => {
                let rows = self.build_settings_rows(SettingsTab::General);
                self.ui_state.modal = Modal::Settings(SettingsState {
                    tab: SettingsTab::General,
                    selected_row: 0,
                    editing: None,
                    rows,
                });
            }
            UserCommand::Quit => {
                self.ui_state.should_quit = true;
            }
            UserCommand::PageUp => self.active_pane_state().page_up(),
            UserCommand::PageDown => self.active_pane_state().page_down(),
            UserCommand::ScrollUp => self.active_pane_state().scroll_up(1),
            UserCommand::ScrollDown => self.active_pane_state().scroll_down(1),
            UserCommand::GenerateSummary => {
                // Context-specific: only works when Info pane is showing
                if self.ui_state.right_pane_view == RightPaneView::Info
                    && let Some(session_id) = self.ui_state.selected_session_id
                {
                    self.spawn_ai_summary_if_needed(session_id);
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
            StateUpdate::PreviewReady {
                session_id,
                project_id,
                preview_content,
                diff_info,
                shell_content,
            } => {
                let elapsed = self
                    .ui_state
                    .preview_update_spawned_at
                    .map(|t| t.elapsed())
                    .unwrap_or_default();
                self.ui_state.preview_update_spawned_at = None;

                // Only apply if selection hasn't changed since the fetch started
                if session_id == self.ui_state.selected_session_id
                    && project_id == self.ui_state.selected_project_id
                {
                    debug!(
                        "Applying PreviewReady (preview_len={} diff_lines={} elapsed={:?})",
                        preview_content.len(),
                        diff_info.line_count,
                        elapsed
                    );
                    self.ui_state.preview_content = preview_content;
                    self.ui_state.diff_info = diff_info;
                    self.ui_state.shell_content = shell_content;
                } else {
                    debug!(
                        "Discarding stale PreviewReady (selection changed, elapsed={:?})",
                        elapsed
                    );
                }
            }
            StateUpdate::PrStatusReady { results } => {
                let _ = self
                    .store
                    .mutate(move |state| {
                        for (session_id, pr_info) in &results {
                            if let Some(session) = state.get_session_mut(session_id) {
                                session.pr_number = pr_info.as_ref().map(|p| p.number);
                                session.pr_url = pr_info.as_ref().map(|p| p.url.clone());
                                session.pr_state = pr_info.as_ref().map(|p| p.state);
                                session.pr_draft = pr_info.as_ref().is_some_and(|p| p.is_draft);
                                session.pr_labels = pr_info
                                    .as_ref()
                                    .map(|p| p.labels.clone())
                                    .unwrap_or_default();
                                session.pr_merged = pr_info.as_ref().is_some_and(|p| p.merged());
                            }
                        }
                    })
                    .await;

                // Update tmux status bars for running sessions with PR info
                {
                    let state = self.store.read().await;
                    for session in state.sessions.values() {
                        if session.status == SessionStatus::Running {
                            let info = self.session_manager.status_bar_info(session, &state);
                            self.session_manager
                                .tmux
                                .configure_status_bar(&session.tmux_session_name, &info)
                                .await;
                        }
                    }
                }

                self.refresh_list_items().await;
            }
            StateUpdate::EnrichedPrReady { session_id, info } => {
                // Only apply if the session is still selected
                if self.ui_state.selected_session_id == Some(session_id) {
                    self.ui_state.enriched_pr = info.map(|pr| (session_id, pr));
                } else {
                    debug!("Discarding stale EnrichedPrReady for {}", session_id);
                }
            }
            StateUpdate::AiSummaryReady {
                session_id,
                result,
                diff_hash: hash,
            } => match result {
                Ok(text) => {
                    self.ui_state.ai_summaries.insert(
                        session_id,
                        AiSummary::Ready {
                            text,
                            diff_hash: hash,
                        },
                    );
                }
                Err(msg) => {
                    self.ui_state
                        .ai_summaries
                        .insert(session_id, AiSummary::Error(msg));
                }
            },
            StateUpdate::SessionCreated { session_id } => {
                debug!("Session created: {}", session_id);
                self.ui_state.modal = Modal::None;
                self.ui_state.status_message = Some((
                    format!("Created session {}", session_id),
                    Instant::now() + Duration::from_secs(3),
                ));
                self.refresh_list_items().await;
                // Select the newly created session
                if let Some(idx) = self.ui_state.list_items.iter().position(|item| {
                    matches!(item, SessionListItem::Worktree { id, .. } if *id == session_id)
                }) {
                    self.ui_state.list_state.select(Some(idx));
                }
                self.update_selection();
                self.spawn_preview_update();
            }
            StateUpdate::SessionCreateFailed {
                session_id,
                message,
            } => {
                debug!("Session creation failed: {}", message);
                let _ = self
                    .session_manager
                    .remove_creating_session(&session_id)
                    .await;
                self.refresh_list_items().await;
                self.ui_state.modal = Modal::Error { message };
            }
            StateUpdate::AgentStatesUpdated { states } => {
                // Detect Working → Idle transitions and mark sessions as unread
                let mut unread_ids = Vec::new();
                for (session_id, new_state) in &states {
                    if *new_state == AgentState::Idle
                        && self.ui_state.agent_states.get(session_id) == Some(&AgentState::Working)
                    {
                        unread_ids.push(*session_id);
                    }
                }
                if !unread_ids.is_empty() {
                    let _ = self
                        .store
                        .mutate(move |state| {
                            for sid in &unread_ids {
                                if let Some(session) = state.get_session_mut(sid) {
                                    session.unread = true;
                                }
                            }
                        })
                        .await;
                }
                self.ui_state.agent_states = states;
                self.refresh_list_items().await;
            }
            StateUpdate::ExternalChange => {
                debug!("External state change detected, refreshing UI");
                self.refresh_list_items().await;
            }
            StateUpdate::Error { message } => {
                self.ui_state.modal = Modal::Error { message };
            }
            _ => {}
        }
    }

    /// Check if the selected session is in Creating state
    fn selected_session_is_creating(&self) -> bool {
        self.ui_state.list_items.iter().any(|item| {
            matches!(
                item,
                SessionListItem::Worktree { id, status, .. }
                if self.ui_state.selected_session_id == Some(*id)
                    && *status == SessionStatus::Creating
            )
        })
    }

    /// Handle selection (attach to session)
    async fn handle_select(&mut self) {
        info!(
            "handle_select called, selected_session_id: {:?}",
            self.ui_state.selected_session_id
        );
        if self.selected_session_is_creating() {
            return;
        }
        if let Some(session_id) = self.ui_state.selected_session_id {
            info!("Getting attach command for session: {}", session_id);
            match self.session_manager.get_attach_command(&session_id).await {
                Ok(cmd) => {
                    info!("Got attach command: {}", cmd);
                    // Clear unread flag when attaching
                    let sid = session_id;
                    let _ = self
                        .store
                        .mutate(move |state| {
                            if let Some(session) = state.get_session_mut(&sid) {
                                session.unread = false;
                            }
                        })
                        .await;
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

    /// Handle shell selection (attach to shell session)
    async fn handle_select_shell(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
        if let Some(session_id) = self.ui_state.selected_session_id {
            match self
                .session_manager
                .get_shell_attach_command(&session_id)
                .await
            {
                Ok(cmd) => {
                    self.ui_state.attach_command = Some(cmd);
                    self.ui_state.should_quit = true;
                }
                Err(e) => {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Cannot open shell: {}", e),
                    };
                }
            }
        } else if let Some(project_id) = self.ui_state.selected_project_id {
            match self
                .session_manager
                .get_project_shell_attach_command(&project_id)
                .await
            {
                Ok(cmd) => {
                    self.ui_state.attach_command = Some(cmd);
                    self.ui_state.should_quit = true;
                }
                Err(e) => {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Cannot open shell: {}", e),
                    };
                }
            }
        }
    }

    /// Resolve the shell toggle pair for a given tmux session name.
    ///
    /// If the current session is a Claude session, returns the shell session name
    /// (creating it if needed). If the current session is already a shell session
    /// (ends with "-sh"), returns the Claude session name.
    async fn resolve_shell_toggle_pair(
        &mut self,
        current_tmux_name: &str,
    ) -> crate::error::Result<String> {
        if current_tmux_name.ends_with("-sh") {
            // We're in a shell session — the Claude session is the name without "-sh"
            let claude_name = current_tmux_name.trim_end_matches("-sh").to_string();
            // Verify the Claude session exists
            if self
                .session_manager
                .tmux
                .session_exists(&claude_name)
                .await?
            {
                return Ok(claude_name);
            }
            return Err(crate::error::Error::Session(
                crate::error::SessionError::TmuxSessionNotFound(claude_name),
            ));
        }

        // We're in a Claude session — find the matching session ID and ensure shell exists
        let session_id = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .find(|s| s.tmux_session_name == current_tmux_name)
                .map(|s| s.id)
        };

        if let Some(session_id) = session_id {
            let shell_name = self
                .session_manager
                .ensure_shell_session(&session_id)
                .await?;
            return Ok(shell_name);
        }

        // Try project-level shell
        let project_id = {
            let state = self.store.read().await;
            state
                .projects
                .values()
                .find(|p| p.shell_tmux_session_name.as_deref() == Some(current_tmux_name))
                .map(|p| p.id)
        };

        if let Some(project_id) = project_id {
            let shell_name = self
                .session_manager
                .ensure_project_shell_session(&project_id)
                .await?;
            return Ok(shell_name);
        }

        Err(crate::error::Error::Session(
            crate::error::SessionError::TmuxSessionNotFound(format!(
                "No session found for tmux name: {}",
                current_tmux_name
            )),
        ))
    }

    /// Open the editor for the worktree associated with a given tmux session
    /// name. Used when the user presses Ctrl+. while attached to a tmux
    /// session — the tmux session itself is not affected, we simply launch
    /// the configured editor pointing at the session's worktree. This runs
    /// while we are *between* attaches, so the TUI is torn down and raw mode
    /// is already disabled.
    async fn open_editor_for_tmux_session(&mut self, tmux_session_name: &str) {
        // Shell sessions are named `<claude_name>-sh`; the worktree is owned
        // by the underlying Claude session.
        let lookup_name = tmux_session_name
            .strip_suffix("-sh")
            .unwrap_or(tmux_session_name)
            .to_string();

        let path = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .find(|s| s.tmux_session_name == lookup_name)
                .map(|s| s.worktree_path.clone())
        };

        let Some(path) = path else {
            warn!(
                "OpenEditor: no session found for tmux name '{}'",
                tmux_session_name
            );
            return;
        };

        let Some(editor) = self.config.resolve_editor() else {
            warn!("OpenEditor: no editor configured");
            return;
        };

        if self.config.is_gui_editor(&editor) {
            // GUI editor: spawn detached and return — tmux session is
            // untouched and we'll re-attach immediately.
            info!("OpenEditor: launching GUI editor '{}' at {}", editor, path.display());
            if let Err(e) = std::process::Command::new(&editor).arg(&path).spawn() {
                warn!("Failed to launch GUI editor '{}': {}", editor, e);
            }
        } else {
            // Terminal editor: run foreground, inheriting stdio. Raw mode is
            // already off (attach_to_session disabled it on exit) so the
            // editor gets a cooked terminal. When it returns we loop back
            // into attach_to_session with the same tmux session name.
            info!(
                "OpenEditor: launching terminal editor '{}' at {}",
                editor,
                path.display()
            );
            if let Err(e) = std::process::Command::new(&editor).arg(&path).status() {
                warn!("Failed to launch terminal editor '{}': {}", editor, e);
            }
        }
    }

    /// Handle open in editor command
    async fn handle_open_in_editor(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
        let path = {
            let state = self.store.read().await;
            if let Some(session_id) = self.ui_state.selected_session_id {
                state
                    .sessions
                    .get(&session_id)
                    .map(|s| s.worktree_path.clone())
            } else if let Some(project_id) = self.ui_state.selected_project_id {
                state.projects.get(&project_id).map(|p| p.repo_path.clone())
            } else {
                None
            }
        };

        let Some(path) = path else {
            return;
        };

        let Some(editor) = self.config.resolve_editor() else {
            self.ui_state.modal = Modal::Error {
                message: "No editor configured. Set 'editor' in config.toml or \
                          set $VISUAL / $EDITOR."
                    .to_string(),
            };
            return;
        };

        if self.config.is_gui_editor(&editor) {
            // GUI editor: spawn detached, TUI stays up
            if let Err(e) = std::process::Command::new(&editor).arg(&path).spawn() {
                self.ui_state.modal = Modal::Error {
                    message: format!("Failed to launch '{}': {}", editor, e),
                };
            }
        } else {
            // Terminal editor: tear down TUI, run foreground, restore
            self.ui_state.editor_command = Some((editor, path));
            self.ui_state.should_quit = true;
        }
    }

    /// Handle "open PR in browser" — looks up the selected session's
    /// `pr_url` and launches the OS default handler (`open` on macOS,
    /// `xdg-open` on Linux, `cmd /c start` on Windows).
    async fn handle_open_pull_request(&mut self) {
        let Some(session_id) = self.ui_state.selected_session_id else {
            return;
        };
        let pr_url = {
            let state = self.store.read().await;
            state
                .sessions
                .get(&session_id)
                .and_then(|s| s.pr_url.clone())
        };
        let Some(url) = pr_url else {
            self.ui_state.status_message = Some((
                "No PR associated with this session".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };

        let result = if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(&url).spawn()
        } else if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/c", "start", "", &url])
                .spawn()
        } else {
            std::process::Command::new("xdg-open").arg(&url).spawn()
        };

        if let Err(e) = result {
            self.ui_state.modal = Modal::Error {
                message: format!("Failed to open PR in browser: {}", e),
            };
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
            self.ui_state.status_message = Some((
                "Select a project first (use N to add one)".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
        }
    }

    /// Open the quick-switch modal with all sessions
    async fn open_quick_switch(&mut self) {
        let matches = self.gather_quick_switch_matches("").await;
        self.ui_state.modal = Modal::QuickSwitch {
            query: String::new(),
            matches,
            selected_idx: 0,
        };
    }

    /// Gather session matches for a query (empty query = all sessions)
    async fn gather_quick_switch_matches(&self, query: &str) -> Vec<QuickSwitchMatch> {
        let state = self.store.read().await;
        let mut matches = Vec::new();

        for session in state.sessions.values() {
            if session.status == SessionStatus::Creating {
                continue;
            }
            if !query.is_empty() && !session.matches_query(query) {
                continue;
            }
            let project_name = state
                .get_project(&session.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            matches.push(QuickSwitchMatch {
                session_id: session.id,
                title: session.title.clone(),
                branch: session.branch.clone(),
                project_name,
                status: session.status,
            });
        }

        // Sort by title for predictable ordering
        matches.sort_by(|a, b| a.title.cmp(&b.title));
        matches
    }

    /// Re-filter the quick-switch matches based on the current query.
    /// Rebuilds from list_items so backspace can widen results.
    fn refilter_quick_switch(&mut self) {
        if let Modal::QuickSwitch {
            query,
            matches,
            selected_idx,
        } = &mut self.ui_state.modal
        {
            let query_lower = query.to_lowercase();
            // Build project name lookup from list items
            let mut project_names: std::collections::HashMap<SessionId, String> =
                std::collections::HashMap::new();
            let mut current_project_name = String::new();
            for item in &self.ui_state.list_items {
                match item {
                    SessionListItem::Project { name, .. } => {
                        current_project_name = name.clone();
                    }
                    SessionListItem::Worktree { id, .. } => {
                        project_names.insert(*id, current_project_name.clone());
                    }
                }
            }

            *matches = self
                .ui_state
                .list_items
                .iter()
                .filter_map(|item| {
                    if let SessionListItem::Worktree {
                        id,
                        title,
                        branch,
                        status,
                        ..
                    } = item
                    {
                        let project_name = project_names.get(id).cloned().unwrap_or_default();
                        if query_lower.is_empty() || title.to_lowercase().contains(&query_lower) {
                            return Some(QuickSwitchMatch {
                                session_id: *id,
                                title: title.clone(),
                                branch: branch.clone(),
                                project_name,
                                status: *status,
                            });
                        }
                    }
                    None
                })
                .collect();

            // Clamp selection
            if *selected_idx >= matches.len() {
                *selected_idx = matches.len().saturating_sub(1);
            }
        }
    }

    /// Handle remove project - show confirmation (only when a project row is selected)
    fn handle_remove_project(&mut self) {
        if self.ui_state.selected_session_id.is_none()
            && let Some(project_id) = self.ui_state.selected_project_id
        {
            self.ui_state.modal = Modal::Confirm {
                    title: "Remove Project".to_string(),
                    message: "Are you sure you want to remove this project?\nThis will kill all sessions and remove all worktrees.".to_string(),
                    on_confirm: ConfirmAction::RemoveProject { project_id },
                };
        }
    }

    /// Handle restart session - show confirmation
    fn handle_restart_session(&mut self) {
        if let Some(session_id) = self.ui_state.selected_session_id {
            let message = if self.config.resume_session {
                "This will kill the current tmux session and start a fresh one.\nClaude will pick up where it left off via /resume.".to_string()
            } else {
                "This will kill the current tmux session and start a fresh one.\nIf you want to pick up where you left off, you can use /resume.".to_string()
            };
            self.ui_state.modal = Modal::Confirm {
                title: "Restart Session".to_string(),
                message,
                on_confirm: ConfirmAction::RestartSession { session_id },
            };
        }
    }

    /// Handle delete session - show confirmation
    fn handle_delete_session(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
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
                    self.ui_state.status_message = Some((
                        "Session name cannot be empty".to_string(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                    return;
                }

                // Insert placeholder session immediately (no blocking modal)
                self.ui_state.modal = Modal::None;
                let session_id = match self
                    .session_manager
                    .prepare_session(&project_id, value, None)
                    .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to create session: {}", e),
                        };
                        return;
                    }
                };

                // Refresh list and select the new placeholder
                self.refresh_list_items().await;
                if let Some(idx) = self.ui_state.list_items.iter().position(|item| {
                    matches!(item, SessionListItem::Worktree { id, .. } if *id == session_id)
                }) {
                    self.ui_state.list_state.select(Some(idx));
                }
                self.update_selection();

                // Spawn background task for heavy work
                let session_manager = self.session_manager.clone();
                let tx = self.event_loop.sender();
                tokio::spawn(async move {
                    match session_manager.finalize_session(&session_id).await {
                        Ok(sid) => {
                            let _ = tx
                                .send(AppEvent::StateUpdate(StateUpdate::SessionCreated {
                                    session_id: sid,
                                }))
                                .await;
                        }
                        Err(e) => {
                            let _ = tx
                                .send(AppEvent::StateUpdate(StateUpdate::SessionCreateFailed {
                                    session_id,
                                    message: format!("Failed to create session: {}", e),
                                }))
                                .await;
                        }
                    }
                });
            }
            InputAction::AddProject => {
                let expanded = super::path_completer::expand_tilde(value.trim());
                let path = PathBuf::from(expanded);
                if !path.exists() {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Path does not exist: {}", path.display()),
                    };
                    return;
                }

                match self.session_manager.add_project(path).await {
                    Ok(project_id) => {
                        self.ui_state.status_message = Some((
                            format!("Added project {}", project_id),
                            Instant::now() + Duration::from_secs(3),
                        ));
                        self.refresh_list_items().await;
                        // Select the newly added project
                        if let Some(idx) = self.ui_state.list_items.iter().position(|item| {
                            matches!(item, SessionListItem::Project { id, .. } if *id == project_id)
                        }) {
                            self.ui_state.list_state.select(Some(idx));
                        }
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
                // 1. Capture session data before removal
                let cleanup_data = {
                    let state = self.store.read().await;
                    state.get_session(&session_id).map(|s| {
                        let repo_path = state
                            .get_project(&s.project_id)
                            .map(|p| p.repo_path.clone());
                        (
                            s.tmux_session_name.clone(),
                            s.shell_tmux_session_name.clone(),
                            s.worktree_path.clone(),
                            repo_path,
                        )
                    })
                };

                // 2. Remove from state immediately so the UI updates
                if let Err(e) = self
                    .store
                    .mutate(move |state| {
                        state.remove_session(&session_id);
                    })
                    .await
                {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Failed to save state: {}", e),
                    };
                    return;
                }
                self.ui_state.selected_session_id = None;
                self.ui_state.status_message = Some((
                    "Session deleted".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
                self.refresh_list_items().await;

                // 3. Spawn background cleanup (kill tmux + remove worktree)
                if let Some((tmux_name, shell_tmux_name, worktree_path, repo_path)) = cleanup_data {
                    let tmux = self.session_manager.tmux.clone();
                    let tx = self.event_loop.sender();
                    tokio::spawn(async move {
                        cleanup_session_tmux(
                            &tmux,
                            &tmux_name,
                            shell_tmux_name.as_deref(),
                            repo_path
                                .as_ref()
                                .map(|rp| (worktree_path.as_path(), rp.as_path())),
                            &tx,
                        )
                        .await;
                    });
                }
            }
            ConfirmAction::RestartSession { session_id } => {
                match self.session_manager.restart_session(&session_id).await {
                    Ok(_) => {
                        self.ui_state.status_message = Some((
                            "Session restarted".to_string(),
                            Instant::now() + Duration::from_secs(3),
                        ));
                        self.refresh_list_items().await;
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to restart: {}", e),
                        };
                    }
                }
            }
            ConfirmAction::RemoveProject { project_id } => {
                // 1. Capture project and session data before removal
                let cleanup_data = {
                    let state = self.store.read().await;
                    state.get_project(&project_id).map(|project| {
                        let repo_path = project.repo_path.clone();
                        let shell_tmux = project.shell_tmux_session_name.clone();
                        let sessions: Vec<_> = project
                            .worktrees
                            .iter()
                            .filter_map(|sid| {
                                state.get_session(sid).map(|s| {
                                    (
                                        s.tmux_session_name.clone(),
                                        s.shell_tmux_session_name.clone(),
                                        s.worktree_path.clone(),
                                    )
                                })
                            })
                            .collect();
                        (repo_path, shell_tmux, sessions)
                    })
                };

                // 2. Remove from state immediately so the UI updates
                if let Err(e) = self
                    .store
                    .mutate(move |state| {
                        state.remove_project(&project_id);
                    })
                    .await
                {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Failed to save state: {}", e),
                    };
                    return;
                }
                self.ui_state.selected_project_id = None;
                self.ui_state.status_message = Some((
                    "Project removed".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
                self.refresh_list_items().await;

                // 3. Spawn background cleanup (kill all tmux sessions + remove worktrees)
                if let Some((repo_path, shell_tmux, sessions)) = cleanup_data {
                    let tmux = self.session_manager.tmux.clone();
                    let tx = self.event_loop.sender();
                    tokio::spawn(async move {
                        // Kill project shell tmux session
                        if let Some(ref shell_name) = shell_tmux {
                            let _ = tmux.kill_session(shell_name).await;
                        }
                        // Kill all session tmux sessions + remove worktrees
                        for (tmux_name, shell_tmux_name, worktree_path) in &sessions {
                            cleanup_session_tmux(
                                &tmux,
                                tmux_name,
                                shell_tmux_name.as_deref(),
                                Some((worktree_path.as_path(), repo_path.as_path())),
                                &tx,
                            )
                            .await;
                        }
                    });
                }
            }
        }
    }

    /// Spawn a background task to check PR status for all sessions
    fn spawn_pr_status_check(&mut self) {
        self.ui_state.last_pr_check = Some(Instant::now());

        let store = self.store.clone();
        let tx = self.event_loop.sender();

        tokio::spawn(async move {
            // Collect session info under a brief read lock
            let sessions_to_check: Vec<(SessionId, String, std::path::PathBuf)> = {
                let state = store.read().await;
                state
                    .sessions
                    .values()
                    .filter(|s| s.status != SessionStatus::Creating)
                    .filter_map(|s| {
                        let project = state.projects.get(&s.project_id)?;
                        Some((s.id, s.branch.clone(), project.repo_path.clone()))
                    })
                    .collect()
            };

            let results = futures::future::join_all(sessions_to_check.into_iter().map(
                |(session_id, branch, repo_path)| async move {
                    let pr_info = check_pr_for_branch(&repo_path, &branch).await;
                    (session_id, pr_info)
                },
            ))
            .await;

            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::PrStatusReady {
                    results,
                }))
                .await;
        });
    }

    /// Spawn background fetches for info pane data (enriched PR + AI summary).
    ///
    /// Only called from user-initiated actions (pane switch, selection change).
    /// Not called from background ticks to avoid unnecessary regeneration.
    fn spawn_info_fetch(&mut self) {
        // Only relevant when the Info pane is active
        if self.ui_state.right_pane_view != RightPaneView::Info {
            return;
        }

        let Some(session_id) = self.ui_state.selected_session_id else {
            return;
        };

        // Find the session's PR number and project repo path
        let session_info = self.ui_state.list_items.iter().find_map(|item| {
            if let SessionListItem::Worktree { id, pr_number, .. } = item {
                if *id == session_id {
                    Some(*pr_number)
                } else {
                    None
                }
            } else {
                None
            }
        });

        let Some(pr_number) = session_info.flatten() else {
            // No PR for this session — skip enriched PR fetch
            return;
        };

        // Spawn enriched PR fetch if not already cached for this session
        let needs_enriched = !self
            .ui_state
            .enriched_pr
            .as_ref()
            .is_some_and(|(sid, _)| *sid == session_id);

        if needs_enriched && self.ui_state.gh_available {
            let store = self.store.clone();
            let tx = self.event_loop.sender();

            tokio::spawn(async move {
                // Look up the project repo path
                let repo_path = {
                    let state = store.read().await;
                    state
                        .sessions
                        .get(&session_id)
                        .and_then(|s| state.projects.get(&s.project_id))
                        .map(|p| p.repo_path.clone())
                };

                let info = if let Some(repo_path) = repo_path {
                    fetch_enriched_pr(&repo_path, pr_number).await
                } else {
                    None
                };

                let _ = tx
                    .send(AppEvent::StateUpdate(StateUpdate::EnrichedPrReady {
                        session_id,
                        info,
                    }))
                    .await;
            });
        }
    }

    /// Spawn AI summary generation for the given session.
    ///
    /// Called from the `GenerateSummary` hotkey handler. Always generates
    /// (unless already in flight or AI is disabled). Computes a full branch
    /// diff (committed vs main + uncommitted) and pipes it into Claude.
    fn spawn_ai_summary_if_needed(&mut self, session_id: SessionId) {
        if !self.config.ai_summary_enabled {
            return;
        }

        // Don't spawn if already in flight
        if matches!(
            self.ui_state.ai_summaries.get(&session_id),
            Some(AiSummary::Loading)
        ) {
            return;
        }

        self.ui_state
            .ai_summaries
            .insert(session_id, AiSummary::Loading);

        let store = self.store.clone();
        let model = self.config.ai_summary_model.clone();
        let tx = self.event_loop.sender();

        tokio::spawn(async move {
            let session_info = {
                let state = store.read().await;
                state.sessions.get(&session_id).and_then(|s| {
                    let project = state.projects.get(&s.project_id)?;
                    Some((s.worktree_path.clone(), project.main_branch.clone()))
                })
            };

            let result = if let Some((worktree_path, main_branch)) = session_info {
                let diff_text = crate::git::compute_branch_diff(&worktree_path, &main_branch).await;
                let new_hash = diff_hash(&diff_text);
                let summary_result = fetch_branch_summary(&diff_text, &model).await;
                (summary_result, new_hash)
            } else {
                (Err("Session not found".to_string()), 0)
            };

            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::AiSummaryReady {
                    session_id,
                    result: result.0,
                    diff_hash: result.1,
                }))
                .await;
        });
    }

    /// Refresh the list items from app state
    async fn refresh_list_items(&mut self) {
        let state = self.store.read().await;

        let mut items = Vec::new();

        // Build hierarchical list with stable sort order
        let mut projects: Vec<_> = state.projects.values().collect();
        projects.sort_by(|a, b| a.name.cmp(&b.name));

        for project in projects {
            // Add project item
            items.push(SessionListItem::Project {
                id: project.id,
                name: project.name.clone(),
                repo_path: project.repo_path.clone(),
                main_branch: project.main_branch.clone(),
                worktree_count: project.worktrees.len(),
            });

            // Add worktree sessions sorted by creation time (newest first)
            let mut sessions: Vec<_> = project
                .worktrees
                .iter()
                .filter_map(|sid| state.sessions.get(sid))
                .collect();
            sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            for session in sessions {
                items.push(SessionListItem::Worktree {
                    id: session.id,
                    project_id: session.project_id,
                    title: session.title.clone(),
                    branch: session.branch.clone(),
                    status: session.status,
                    program: session.program.clone(),
                    pr_number: session.pr_number,
                    pr_url: session.pr_url.clone(),
                    pr_merged: session.pr_merged,
                    pr_state: session.pr_state,
                    pr_draft: session.pr_draft,
                    pr_labels: session.pr_labels.clone(),
                    worktree_path: session.worktree_path.clone(),
                    created_at: session.created_at,
                    agent_state: self.ui_state.agent_states.get(&session.id).copied(),
                    unread: session.unread,
                });
            }
        }

        self.ui_state.list_items = items;
        self.ui_state
            .list_state
            .set_item_count(self.ui_state.list_items.len());

        // Clear status message after a bit
        // (In a real app, you'd use a timer)
    }

    /// Save current selection to persisted state
    async fn save_selection(&self) {
        let session_id = self.ui_state.selected_session_id;
        let project_id = self.ui_state.selected_project_id;
        let _ = self
            .store
            .mutate(move |state| {
                state.last_selected_session = session_id;
                state.last_selected_project = project_id;
            })
            .await;
    }

    /// Save left pane width to persisted state
    async fn save_left_pane_pct(&self) {
        let pct = self.ui_state.left_pane_pct;
        let _ = self
            .store
            .mutate(move |state| {
                state.left_pane_pct = Some(pct);
            })
            .await;
    }

    /// Restore selection and UI preferences from persisted state
    async fn restore_selection(&mut self) {
        let (last_session, last_project, left_pane_pct) = {
            let state = self.store.read().await;
            (
                state.last_selected_session,
                state.last_selected_project,
                state.left_pane_pct,
            )
        };

        if let Some(pct) = left_pane_pct {
            self.ui_state.left_pane_pct = pct.clamp(MIN_LEFT_PANE_PCT, MAX_LEFT_PANE_PCT);
        }

        // Try to find the last selected session or project in the list
        let target_idx = self.ui_state.list_items.iter().position(|item| match item {
            SessionListItem::Worktree { id, .. } => last_session.is_some_and(|s| s == *id),
            SessionListItem::Project { id, .. } => {
                last_session.is_none() && last_project.is_some_and(|p| p == *id)
            }
        });

        if let Some(idx) = target_idx {
            self.ui_state.list_state.select(Some(idx));
        } else if !self.ui_state.list_items.is_empty() {
            self.ui_state.list_state.select(Some(0));
        }
    }
}

/// Fetch preview/diff/shell data for the currently selected session or project.
///
/// Runs outside the main event loop so it never blocks keyboard input.
async fn fetch_preview_data(
    mgr: &SessionManager,
    session_id: Option<SessionId>,
    project_id: Option<ProjectId>,
) -> (String, Arc<DiffInfo>, String) {
    if let Some(sid) = session_id {
        // Check if session is still Creating (no tmux session to capture yet)
        let is_creating = {
            let state = mgr.store.read().await;
            state
                .get_session(&sid)
                .is_some_and(|s| s.status == SessionStatus::Creating)
        };
        if is_creating {
            return (
                "Creating session...".to_string(),
                Arc::new(DiffInfo::empty()),
                String::new(),
            );
        }

        debug!(
            "fetch_preview_data: fetching content/diff/shell for session {}",
            sid
        );
        let (preview_result, diff_result, shell_result) = tokio::join!(
            mgr.get_content(&sid),
            mgr.get_diff(&sid),
            mgr.get_shell_content(&sid),
        );

        let preview = preview_result.map(|c| c.content).unwrap_or_else(|e| {
            debug!("fetch_preview_data: get_content error: {}", e);
            "Unable to capture content".to_string()
        });
        let diff = diff_result.unwrap_or_else(|e| {
            debug!("fetch_preview_data: get_diff error: {}", e);
            Arc::new(DiffInfo::empty())
        });
        let shell = match shell_result {
            Ok(Some(c)) => c.content,
            Ok(None) => "No shell session. Press 's' to open one.".to_string(),
            Err(e) => {
                debug!("fetch_preview_data: get_shell_content error: {}", e);
                "No shell session. Press 's' to open one.".to_string()
            }
        };

        (preview, diff, shell)
    } else if let Some(pid) = project_id {
        debug!(
            "fetch_preview_data: fetching diff/shell for project {}",
            pid
        );
        let (diff_result, shell_result) = tokio::join!(
            mgr.get_project_diff(&pid),
            mgr.get_project_shell_content(&pid),
        );

        let diff = diff_result.unwrap_or_else(|e| {
            debug!("fetch_preview_data: get_project_diff error: {}", e);
            Arc::new(DiffInfo::empty())
        });
        let shell = match shell_result {
            Ok(Some(c)) => c.content,
            _ => "No shell session. Press 's' to open one.".to_string(),
        };

        (String::new(), diff, shell)
    } else {
        debug!("fetch_preview_data: no selection");
        (
            "Select a session to see preview".to_string(),
            Arc::new(DiffInfo::empty()),
            String::new(),
        )
    }
}

/// Format a ratatui Color for display in the settings modal.
fn format_color(color: ratatui::style::Color) -> String {
    use ratatui::style::Color;
    match color {
        Color::Reset => "reset".into(),
        Color::Black => "black".into(),
        Color::Red => "red".into(),
        Color::Green => "green".into(),
        Color::Yellow => "yellow".into(),
        Color::Blue => "blue".into(),
        Color::Magenta => "magenta".into(),
        Color::Cyan => "cyan".into(),
        Color::Gray => "gray".into(),
        Color::DarkGray => "dark_gray".into(),
        Color::LightRed => "light_red".into(),
        Color::LightGreen => "light_green".into(),
        Color::LightYellow => "light_yellow".into(),
        Color::LightBlue => "light_blue".into(),
        Color::LightMagenta => "light_magenta".into(),
        Color::LightCyan => "light_cyan".into(),
        Color::White => "white".into(),
        Color::Indexed(i) => format!("{i}"),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

/// Kill tmux sessions and remove a git worktree in the background.
///
/// Sends an error event if worktree removal fails.
async fn cleanup_session_tmux(
    tmux: &crate::tmux::TmuxExecutor,
    tmux_name: &str,
    shell_tmux_name: Option<&str>,
    worktree_path: Option<(&std::path::Path, &std::path::Path)>,
    tx: &tokio::sync::mpsc::Sender<AppEvent>,
) {
    if let Err(e) = tmux.kill_session(tmux_name).await {
        debug!("Failed to kill tmux session: {}", e);
    }
    if let Some(shell_name) = shell_tmux_name {
        let _ = tmux.kill_session(shell_name).await;
    }
    if let Some((worktree_path, repo_path)) = worktree_path {
        let output = tokio::process::Command::new("git")
            .current_dir(repo_path)
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .output()
            .await;
        if let Err(e) = output.as_ref().map_err(|e| e.to_string()).and_then(|o| {
            if o.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&o.stderr).into_owned())
            }
        }) {
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::Error {
                    message: format!("Background cleanup failed: {}", e),
                }))
                .await;
        }
    }
}

/// Map a 1-based session number to its index in the flat list_items vec.
/// Returns None if the number is out of range.
fn session_number_to_list_index(items: &[SessionListItem], number: usize) -> Option<usize> {
    let mut count = 0usize;
    for (idx, item) in items.iter().enumerate() {
        if matches!(item, SessionListItem::Worktree { .. }) {
            count += 1;
            if count == number {
                return Some(idx);
            }
        }
    }
    None
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

    fn make_project() -> SessionListItem {
        SessionListItem::Project {
            id: ProjectId::new(),
            name: "test".to_string(),
            repo_path: std::path::PathBuf::from("/tmp/test"),
            main_branch: "main".to_string(),
            worktree_count: 0,
        }
    }

    fn make_worktree() -> SessionListItem {
        SessionListItem::Worktree {
            id: SessionId::new(),
            project_id: ProjectId::new(),
            title: "test".to_string(),
            branch: "feat".to_string(),
            status: SessionStatus::Running,
            program: "claude".to_string(),
            pr_number: None,
            pr_url: None,
            pr_merged: false,
            pr_state: None,
            pr_draft: false,
            pr_labels: Vec::new(),
            worktree_path: std::path::PathBuf::from("/tmp/test"),
            created_at: chrono::Utc::now(),
            agent_state: None,
            unread: false,
        }
    }

    #[test]
    fn test_session_number_to_list_index_basic() {
        let items = vec![
            make_project(),
            make_worktree(), // index 1, session #1
            make_worktree(), // index 2, session #2
            make_project(),
            make_worktree(), // index 4, session #3
        ];
        assert_eq!(session_number_to_list_index(&items, 1), Some(1));
        assert_eq!(session_number_to_list_index(&items, 2), Some(2));
        assert_eq!(session_number_to_list_index(&items, 3), Some(4));
    }

    #[test]
    fn test_session_number_to_list_index_out_of_range() {
        let items = vec![make_project(), make_worktree()];
        assert_eq!(session_number_to_list_index(&items, 2), None);
        assert_eq!(session_number_to_list_index(&items, 0), None);
    }

    #[test]
    fn test_session_number_to_list_index_empty() {
        let items: Vec<SessionListItem> = vec![];
        assert_eq!(session_number_to_list_index(&items, 1), None);
    }

    #[test]
    fn test_session_number_to_list_index_projects_only() {
        let items = vec![make_project(), make_project()];
        assert_eq!(session_number_to_list_index(&items, 1), None);
    }
}
