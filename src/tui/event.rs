//! Event handling for the TUI
//!
//! Provides an async event stream that combines:
//! - Terminal input events (keyboard, mouse)
//! - Application state updates
//! - Render ticks

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tracing::debug;

use crate::session::{ProjectId, SessionId};

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
    StatusChanged {
        session_id: SessionId,
    },
    /// Agent state changed
    AgentStateChanged {
        session_id: SessionId,
    },
    /// Diff updated
    DiffUpdated {
        session_id: SessionId,
    },
    /// Project added
    ProjectAdded {
        project_id: ProjectId,
    },
    /// Session added
    SessionAdded {
        session_id: SessionId,
    },
    /// Session removed
    SessionRemoved {
        session_id: SessionId,
    },
    /// Error occurred
    Error {
        message: String,
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
    /// Toggle between preview/diff panes
    TogglePane,
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
        match (key.code, key.modifiers) {
            // Navigation
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                Some(UserCommand::NavigateUp)
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                Some(UserCommand::NavigateDown)
            }

            // Selection
            (KeyCode::Enter, _) => Some(UserCommand::Select),

            // Session management
            (KeyCode::Char('n'), KeyModifiers::NONE) => Some(UserCommand::NewSession),
            (KeyCode::Char('N'), KeyModifiers::SHIFT) => Some(UserCommand::NewProject),
            (KeyCode::Char('p'), KeyModifiers::NONE) => Some(UserCommand::PauseSession),
            (KeyCode::Char('r'), KeyModifiers::NONE) => Some(UserCommand::ResumeSession),
            (KeyCode::Char('d'), KeyModifiers::NONE) => Some(UserCommand::DeleteSession),

            // Pane control
            (KeyCode::Tab, _) => Some(UserCommand::TogglePane),

            // Scrolling
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => Some(UserCommand::PageUp),
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some(UserCommand::PageDown),
            (KeyCode::PageUp, _) => Some(UserCommand::PageUp),
            (KeyCode::PageDown, _) => Some(UserCommand::PageDown),

            // Help and quit
            (KeyCode::Char('?'), _) => Some(UserCommand::ShowHelp),
            (KeyCode::Char('q'), KeyModifiers::NONE | KeyModifiers::CONTROL) => Some(UserCommand::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(UserCommand::Quit),

            // Modal controls
            (KeyCode::Esc, _) => Some(UserCommand::Cancel),
            (KeyCode::Backspace, _) => Some(UserCommand::Backspace),

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
    /// Flag to pause input reading (during tmux attach)
    input_paused: Arc<AtomicBool>,
}

impl EventLoop {
    /// Create a new event loop
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            tx,
            rx,
            input_paused: Arc::new(AtomicBool::new(false)),
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
        let tx = self.tx.clone();
        let paused = self.input_paused.clone();

        // Terminal input task - single long-running reader
        tokio::spawn(async move {
            let mut reader = EventStream::new();

            loop {
                // When paused, just consume and discard events
                // This keeps the EventStream active but doesn't forward events
                let is_paused = paused.load(Ordering::SeqCst);

                let event = reader.next().fuse().await;

                match event {
                    Some(Ok(event)) => {
                        // Skip events while paused
                        if is_paused {
                            continue;
                        }

                        let app_event = match event {
                            CrosstermEvent::Key(key) => AppEvent::Input(InputEvent::Key(key)),
                            CrosstermEvent::Mouse(mouse) => {
                                AppEvent::Input(InputEvent::Mouse(mouse))
                            }
                            CrosstermEvent::Resize(w, h) => {
                                AppEvent::Input(InputEvent::Resize(w, h))
                            }
                            _ => continue,
                        };

                        if tx.send(app_event).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        debug!("Error reading terminal event: {}", e);
                        // Don't break on error - might be temporary during attach/detach
                        continue;
                    }
                    None => break,
                }
            }
        });

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

    /// Pause input reading (call before tmux attach)
    pub fn pause_input(&self) {
        self.input_paused.store(true, Ordering::SeqCst);
        debug!("Input reading paused");
    }

    /// Resume input reading (call after tmux detach)
    pub fn resume_input(&mut self) {
        // Drain any stale events from the channel
        while self.rx.try_recv().is_ok() {}

        self.input_paused.store(false, Ordering::SeqCst);
        debug!("Input reading resumed");
    }

    /// Receive the next event
    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
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
}
