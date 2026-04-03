//! Event handling for the TUI
//!
//! Provides an async event stream that combines:
//! - Terminal input events (keyboard, mouse)
//! - Application state updates
//! - Render ticks

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::git::DiffInfo;
use crate::session::{ProjectId, SessionId};

use crossterm::event::{
    Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tracing::debug;

/// Application events
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Terminal input event
    Input(InputEvent),
    /// State update from background task
    StateUpdate(StateUpdate),
    /// Render tick
    Tick,
    /// Request to quit the application
    Quit,
}

/// Input events from the terminal
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// Key press
    Key(KeyEvent),
    /// Mouse event (if enabled)
    Mouse(crossterm::event::MouseEvent),
    /// Terminal resize
    Resize(u16, u16),
    /// Bracketed paste
    Paste(String),
}

/// State updates from background tasks
#[derive(Debug, Clone)]
pub enum StateUpdate {
    /// Session content updated
    ContentUpdated {
        session_id: SessionId,
        content_hash: u64,
    },
    /// Session status changed
    StatusChanged { session_id: SessionId },
    /// Diff updated
    DiffUpdated { session_id: SessionId },
    /// Project added
    ProjectAdded { project_id: ProjectId },
    /// Session added
    SessionAdded { session_id: SessionId },
    /// Session removed
    SessionRemoved { session_id: SessionId },
    /// Error occurred
    Error { message: String },
    /// PR status results ready from background check
    PrStatusReady {
        results: Vec<(SessionId, Option<crate::git::PrInfo>)>,
    },
    /// Session creation completed successfully
    SessionCreated { session_id: SessionId },
    /// Session creation failed
    SessionCreateFailed { message: String },
    /// State file was modified by another instance
    ExternalChange,
    /// Preview/diff/shell data ready from background fetch
    PreviewReady {
        /// Which session this data is for (None if project-level)
        session_id: Option<SessionId>,
        /// Which project this data is for
        project_id: Option<ProjectId>,
        /// Preview pane content (tmux capture)
        preview_content: String,
        /// Diff information
        diff_info: Arc<DiffInfo>,
        /// Shell pane content
        shell_content: String,
    },
}

/// User commands triggered by input
#[derive(Debug, Clone)]
pub enum UserCommand {
    /// Navigate up in the list
    NavigateUp,
    /// Navigate down in the list
    NavigateDown,
    /// Select/attach to current item
    Select,
    /// Open shell in worktree
    SelectShell,
    /// Create new session
    NewSession,
    /// Create new project
    NewProject,
    /// Pause current session
    PauseSession,
    /// Resume current session
    ResumeSession,
    /// Delete/kill current session
    DeleteSession,
    /// Remove an entire project
    RemoveProject,
    /// Open worktree in editor/IDE
    OpenInEditor,
    /// Toggle between preview/diff panes
    TogglePane,
    /// Toggle between preview/diff panes (reverse)
    TogglePaneReverse,
    /// Shrink left pane (move divider left)
    ShrinkLeftPane,
    /// Grow left pane (move divider right)
    GrowLeftPane,
    /// Show help
    ShowHelp,
    /// Quit application
    Quit,
    /// Cancel current operation
    Cancel,
    /// Confirm current operation
    Confirm,
    /// Text input
    TextInput(char),
    /// Backspace in text input
    Backspace,
    /// Scroll preview up
    ScrollUp,
    /// Scroll preview down
    ScrollDown,
    /// Page up in preview
    PageUp,
    /// Page down in preview
    PageDown,
}

impl UserCommand {
    /// Convert a key event to a user command
    pub fn from_key(key: KeyEvent) -> Option<Self> {
        // Only process key press events; ignore release/repeat from terminals
        // that support the kitty keyboard protocol
        if key.kind != KeyEventKind::Press {
            return None;
        }

        match (key.code, key.modifiers) {
            // Navigation
            (KeyCode::Up, KeyModifiers::NONE)
            | (KeyCode::Char('k'), KeyModifiers::NONE)
            | (KeyCode::Char('p'), KeyModifiers::CONTROL) => Some(UserCommand::NavigateUp),
            (KeyCode::Down, KeyModifiers::NONE)
            | (KeyCode::Char('j'), KeyModifiers::NONE)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => Some(UserCommand::NavigateDown),

            // Selection
            (KeyCode::Enter, KeyModifiers::NONE) => Some(UserCommand::Select),

            // Session management
            (KeyCode::Char('s'), KeyModifiers::NONE) => Some(UserCommand::SelectShell),
            (KeyCode::Char('n'), KeyModifiers::NONE) => Some(UserCommand::NewSession),
            (KeyCode::Char('N'), KeyModifiers::SHIFT) => Some(UserCommand::NewProject),
            (KeyCode::Char('p'), KeyModifiers::NONE) => Some(UserCommand::PauseSession),
            (KeyCode::Char('r'), KeyModifiers::NONE) => Some(UserCommand::ResumeSession),
            (KeyCode::Char('d'), KeyModifiers::NONE) => Some(UserCommand::DeleteSession),
            (KeyCode::Char('D'), KeyModifiers::SHIFT) => Some(UserCommand::RemoveProject),
            (KeyCode::Char('e'), KeyModifiers::NONE) => Some(UserCommand::OpenInEditor),

            // Pane control
            (KeyCode::Tab, KeyModifiers::NONE) => Some(UserCommand::TogglePane),
            (KeyCode::BackTab, _) => Some(UserCommand::TogglePaneReverse),
            (KeyCode::Char('<'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(UserCommand::ShrinkLeftPane)
            }
            (KeyCode::Char('>'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(UserCommand::GrowLeftPane)
            }

            // Scrolling
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => Some(UserCommand::PageUp),
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some(UserCommand::PageDown),
            (KeyCode::PageUp, KeyModifiers::NONE) => Some(UserCommand::PageUp),
            (KeyCode::PageDown, KeyModifiers::NONE) => Some(UserCommand::PageDown),

            // Help and quit
            (KeyCode::Char('?'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(UserCommand::ShowHelp)
            }
            (KeyCode::Char('q'), KeyModifiers::NONE) => Some(UserCommand::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(UserCommand::Quit),

            // Modal controls
            (KeyCode::Esc, KeyModifiers::NONE) => Some(UserCommand::Cancel),
            (KeyCode::Backspace, KeyModifiers::NONE) => Some(UserCommand::Backspace),

            // Text input (for modals)
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(UserCommand::TextInput(c))
            }

            _ => None,
        }
    }
}

/// Event loop handle
pub struct EventLoop {
    /// Sender for events
    tx: mpsc::Sender<AppEvent>,
    /// Receiver for events
    rx: mpsc::Receiver<AppEvent>,
    /// Generation counter for input reader (used to stop old readers)
    input_generation: Arc<AtomicU64>,
    /// Current tick rate
    tick_rate: Option<Duration>,
}

impl EventLoop {
    /// Create a new event loop
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            tx,
            rx,
            input_generation: Arc::new(AtomicU64::new(0)),
            tick_rate: None,
        }
    }

    /// Get a sender for posting events
    pub fn sender(&self) -> mpsc::Sender<AppEvent> {
        self.tx.clone()
    }

    /// Start the event loop
    ///
    /// This spawns background tasks for:
    /// - Terminal input
    /// - Render ticks
    pub fn start(&mut self, tick_rate: Duration) {
        self.tick_rate = Some(tick_rate);
        self.start_input_reader();

        // Render tick task
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tick_rate);

            loop {
                interval.tick().await;
                if tx.send(AppEvent::Tick).await.is_err() {
                    break;
                }
            }
        });
    }

    /// Start the input reader task
    fn start_input_reader(&self) {
        let tx = self.tx.clone();
        let generation = self.input_generation.load(Ordering::SeqCst);
        let generation_ref = self.input_generation.clone();

        tokio::spawn(async move {
            let mut reader = EventStream::new();

            loop {
                // Check if we should stop (generation changed = stop signal)
                if generation_ref.load(Ordering::SeqCst) != generation {
                    debug!("Input reader stopping (generation changed)");
                    break;
                }

                // Use short timeout to check generation frequently
                let event =
                    tokio::time::timeout(Duration::from_millis(50), reader.next().fuse()).await;

                match event {
                    Ok(Some(Ok(event))) => {
                        // Re-check generation before sending (might have changed during read)
                        if generation_ref.load(Ordering::SeqCst) != generation {
                            debug!("Input reader stopping (generation changed during read)");
                            break;
                        }

                        let app_event = match event {
                            CrosstermEvent::Key(key) => AppEvent::Input(InputEvent::Key(key)),
                            CrosstermEvent::Mouse(mouse) => {
                                AppEvent::Input(InputEvent::Mouse(mouse))
                            }
                            CrosstermEvent::Resize(w, h) => {
                                AppEvent::Input(InputEvent::Resize(w, h))
                            }
                            CrosstermEvent::Paste(text) => AppEvent::Input(InputEvent::Paste(text)),
                            _ => continue,
                        };

                        if tx.send(app_event).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(Err(e))) => {
                        debug!("Error reading terminal event: {}", e);
                        continue;
                    }
                    Ok(None) => break,
                    Err(_) => continue, // Timeout, loop back to check generation
                }
            }
            debug!("Input reader task exited");
        });
    }

    /// Stop the input reader before tmux attach
    ///
    /// Increments generation to signal current reader to stop, then waits briefly
    /// for it to actually stop so it won't compete for stdin during attach.
    pub fn stop_input(&mut self) {
        self.input_generation.fetch_add(1, Ordering::SeqCst);
        debug!("Input reader stop signaled");
    }

    /// Restart the input reader after returning from tmux attach
    pub fn restart_input(&mut self) {
        // Drain any stale events from the channel
        while self.rx.try_recv().is_ok() {}

        // Start a fresh input reader (generation was already incremented by stop_input)
        self.start_input_reader();
        debug!("Input reader restarted");
    }

    /// Receive the next event
    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }

    /// Try to receive an event without blocking
    pub fn try_next(&mut self) -> Option<AppEvent> {
        self.rx.try_recv().ok()
    }

    /// Post a state update
    pub async fn post_update(&self, update: StateUpdate) {
        let _ = self.tx.send(AppEvent::StateUpdate(update)).await;
    }
}

impl Default for EventLoop {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_to_command() {
        // Navigation
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::NavigateDown)
        ));

        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::NavigateUp)
        ));

        // Quit
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::Quit)
        ));

        // Text input
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::TextInput('a'))
        ));
    }

    #[test]
    fn test_pane_resize_keys() {
        let key = KeyEvent::new(KeyCode::Char('<'), KeyModifiers::SHIFT);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::ShrinkLeftPane)
        ));

        let key = KeyEvent::new(KeyCode::Char('>'), KeyModifiers::SHIFT);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::GrowLeftPane)
        ));

        // Some terminals report without SHIFT
        let key = KeyEvent::new(KeyCode::Char('<'), KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::ShrinkLeftPane)
        ));

        let key = KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::GrowLeftPane)
        ));
    }

    #[test]
    fn test_ctrl_c_quits() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::Quit)
        ));
    }

    #[test]
    fn test_ctrl_p_navigates_up() {
        let key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::NavigateUp)
        ));
    }

    #[test]
    fn test_ctrl_n_navigates_down() {
        let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::NavigateDown)
        ));
    }

    #[test]
    fn test_arrow_keys() {
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(up),
            Some(UserCommand::NavigateUp)
        ));

        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(down),
            Some(UserCommand::NavigateDown)
        ));
    }

    #[test]
    fn test_enter_selects() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::Select)
        ));
    }

    #[test]
    fn test_tab_toggles_pane() {
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(tab),
            Some(UserCommand::TogglePane)
        ));

        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert!(matches!(
            UserCommand::from_key(backtab),
            Some(UserCommand::TogglePaneReverse)
        ));
    }

    #[test]
    fn test_session_management_keys() {
        let cases: Vec<(KeyCode, KeyModifiers, UserCommand)> = vec![
            (
                KeyCode::Char('s'),
                KeyModifiers::NONE,
                UserCommand::SelectShell,
            ),
            (
                KeyCode::Char('n'),
                KeyModifiers::NONE,
                UserCommand::NewSession,
            ),
            (
                KeyCode::Char('N'),
                KeyModifiers::SHIFT,
                UserCommand::NewProject,
            ),
            (
                KeyCode::Char('p'),
                KeyModifiers::NONE,
                UserCommand::PauseSession,
            ),
            (
                KeyCode::Char('r'),
                KeyModifiers::NONE,
                UserCommand::ResumeSession,
            ),
            (
                KeyCode::Char('d'),
                KeyModifiers::NONE,
                UserCommand::DeleteSession,
            ),
            (
                KeyCode::Char('D'),
                KeyModifiers::SHIFT,
                UserCommand::RemoveProject,
            ),
            (
                KeyCode::Char('e'),
                KeyModifiers::NONE,
                UserCommand::OpenInEditor,
            ),
        ];

        for (code, modifiers, expected) in cases {
            let key = KeyEvent::new(code, modifiers);
            let result = UserCommand::from_key(key);
            assert!(
                result.is_some(),
                "Expected Some for {:?}+{:?}",
                code,
                modifiers
            );
            assert_eq!(
                std::mem::discriminant(&result.unwrap()),
                std::mem::discriminant(&expected),
                "Mismatch for {:?}+{:?}",
                code,
                modifiers
            );
        }
    }

    #[test]
    fn test_scroll_keys() {
        let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(matches!(
            UserCommand::from_key(ctrl_u),
            Some(UserCommand::PageUp)
        ));

        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(matches!(
            UserCommand::from_key(ctrl_d),
            Some(UserCommand::PageDown)
        ));

        let pgup = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(pgup),
            Some(UserCommand::PageUp)
        ));

        let pgdown = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(pgdown),
            Some(UserCommand::PageDown)
        ));
    }

    #[test]
    fn test_help_key() {
        let q_none = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(q_none),
            Some(UserCommand::ShowHelp)
        ));

        let q_shift = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        assert!(matches!(
            UserCommand::from_key(q_shift),
            Some(UserCommand::ShowHelp)
        ));
    }

    #[test]
    fn test_escape_cancels() {
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::Cancel)
        ));
    }

    #[test]
    fn test_backspace_key() {
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::Backspace)
        ));
    }

    #[test]
    fn test_key_release_ignored() {
        use crossterm::event::{KeyEventKind, KeyEventState};
        let key = KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::empty(),
        };
        assert!(UserCommand::from_key(key).is_none());
    }

    #[test]
    fn test_key_repeat_ignored() {
        use crossterm::event::{KeyEventKind, KeyEventState};
        let key = KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::empty(),
        };
        assert!(UserCommand::from_key(key).is_none());
    }

    #[test]
    fn test_unknown_key_returns_none() {
        let key = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
        assert!(UserCommand::from_key(key).is_none());
    }

    #[test]
    fn test_text_input_uppercase() {
        let key = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert!(matches!(
            UserCommand::from_key(key),
            Some(UserCommand::TextInput('A'))
        ));
    }
}
