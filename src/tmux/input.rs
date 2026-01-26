//! Non-blocking input forwarding to tmux sessions
//!
//! Provides a queue-based input system that:
//! - Buffers keystrokes for efficient batching
//! - Handles special key sequences
//! - Provides non-blocking send operations

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use tracing::{debug, instrument};

use super::TmuxExecutor;
use crate::error::Result;

/// Input event to send to a tmux session
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// Regular text input
    Text(String),
    /// Special key (Enter, Tab, etc.)
    Key(SpecialKey),
    /// Control character (Ctrl+C, Ctrl+D, etc.)
    Control(char),
    /// Raw tmux key sequence
    Raw(String),
}

/// Special keys that can be sent to tmux
#[derive(Debug, Clone, Copy)]
pub enum SpecialKey {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

impl SpecialKey {
    /// Convert to tmux key sequence
    pub fn to_tmux_keys(&self) -> &'static str {
        match self {
            Self::Enter => "Enter",
            Self::Tab => "Tab",
            Self::Escape => "Escape",
            Self::Backspace => "BSpace",
            Self::Delete => "DC",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PPage",
            Self::PageDown => "NPage",
        }
    }
}

impl InputEvent {
    /// Convert to tmux send-keys argument
    pub fn to_tmux_arg(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Key(k) => k.to_tmux_keys().to_string(),
            Self::Control(c) => format!("C-{}", c),
            Self::Raw(s) => s.clone(),
        }
    }
}

/// Input forwarder for a tmux session
///
/// Provides buffered, non-blocking input forwarding.
pub struct InputForwarder {
    /// Tmux executor
    executor: TmuxExecutor,
    /// Session name
    session_name: String,
    /// Input queue
    queue: Arc<Mutex<VecDeque<InputEvent>>>,
    /// Channel for signaling new input
    tx: mpsc::Sender<()>,
}

impl InputForwarder {
    /// Create a new input forwarder
    pub fn new(executor: TmuxExecutor, session_name: String) -> Self {
        let (tx, mut rx) = mpsc::channel::<()>(32);
        let queue = Arc::new(Mutex::new(VecDeque::new()));

        let forwarder = Self {
            executor,
            session_name,
            queue,
            tx,
        };

        // Spawn background task to process queue
        let queue_clone = forwarder.queue.clone();
        let executor_clone = forwarder.executor.clone();
        let session_clone = forwarder.session_name.clone();

        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                // Process all queued input
                loop {
                    let event = {
                        let mut q = queue_clone.lock().await;
                        q.pop_front()
                    };

                    match event {
                        Some(e) => {
                            let arg = e.to_tmux_arg();
                            if let Err(err) = executor_clone
                                .send_keys(&session_clone, &arg)
                                .await
                            {
                                debug!("Failed to send keys: {}", err);
                            }
                        }
                        None => break,
                    }
                }
            }
        });

        forwarder
    }

    /// Queue an input event (non-blocking)
    #[instrument(skip(self))]
    pub async fn send(&self, event: InputEvent) -> Result<()> {
        {
            let mut queue = self.queue.lock().await;
            queue.push_back(event);
        }

        // Signal the background task
        let _ = self.tx.send(()).await;
        Ok(())
    }

    /// Send text input
    pub async fn send_text(&self, text: &str) -> Result<()> {
        self.send(InputEvent::Text(text.to_string())).await
    }

    /// Send a special key
    pub async fn send_key(&self, key: SpecialKey) -> Result<()> {
        self.send(InputEvent::Key(key)).await
    }

    /// Send a control character
    pub async fn send_control(&self, c: char) -> Result<()> {
        self.send(InputEvent::Control(c)).await
    }

    /// Send text followed by Enter
    pub async fn send_line(&self, text: &str) -> Result<()> {
        self.send_text(text).await?;
        self.send_key(SpecialKey::Enter).await
    }

    /// Get queue length
    pub async fn queue_len(&self) -> usize {
        self.queue.lock().await.len()
    }

    /// Clear the input queue
    pub async fn clear(&self) {
        self.queue.lock().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_special_key_conversion() {
        assert_eq!(SpecialKey::Enter.to_tmux_keys(), "Enter");
        assert_eq!(SpecialKey::Tab.to_tmux_keys(), "Tab");
        assert_eq!(SpecialKey::Backspace.to_tmux_keys(), "BSpace");
    }

    #[test]
    fn test_input_event_conversion() {
        assert_eq!(
            InputEvent::Text("hello".to_string()).to_tmux_arg(),
            "hello"
        );
        assert_eq!(InputEvent::Key(SpecialKey::Enter).to_tmux_arg(), "Enter");
        assert_eq!(InputEvent::Control('c').to_tmux_arg(), "C-c");
    }
}
