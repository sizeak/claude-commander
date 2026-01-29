//! Async tmux command executor with semaphore-controlled concurrency
//!
//! Provides non-blocking tmux command execution with:
//! - Semaphore to limit concurrent commands (default: 16)
//! - Timeout handling
//! - Structured output parsing

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, instrument, warn};

use crate::error::{Result, TmuxError};

/// Default maximum concurrent tmux commands
pub const DEFAULT_MAX_CONCURRENT: usize = 16;

/// Default command timeout
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Async tmux command executor
///
/// Uses a semaphore to limit concurrent tmux commands, preventing
/// resource exhaustion when managing many sessions.
#[derive(Clone)]
pub struct TmuxExecutor {
    /// Semaphore for concurrency control
    semaphore: Arc<Semaphore>,
    /// Command timeout
    timeout: Duration,
}

impl TmuxExecutor {
    /// Create a new executor with default settings
    pub fn new() -> Self {
        Self::with_max_concurrent(DEFAULT_MAX_CONCURRENT)
    }

    /// Create an executor with custom concurrency limit
    pub fn with_max_concurrent(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Set the command timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Check if tmux is installed and accessible
    pub async fn check_installed(&self) -> Result<()> {
        let output = Command::new("tmux")
            .arg("-V")
            .output()
            .await
            .map_err(|_| TmuxError::NotInstalled)?;

        if output.status.success() {
            let version = String::from_utf8_lossy(&output.stdout);
            debug!("tmux version: {}", version.trim());
            Ok(())
        } else {
            Err(TmuxError::NotInstalled.into())
        }
    }

    /// Execute a tmux command and return its output
    #[instrument(skip(self), fields(args = ?args))]
    pub async fn execute(&self, args: &[&str]) -> Result<String> {
        // Acquire semaphore permit
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| TmuxError::SemaphoreError)?;

        // Build command
        let mut cmd = Command::new("tmux");
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Execute with timeout
        let result = timeout(self.timeout, cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    Ok(String::from_utf8_lossy(&output.stdout).to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    Err(TmuxError::CommandFailed {
                        command: format!("tmux {}", args.join(" ")),
                        stderr,
                    }
                    .into())
                }
            }
            Ok(Err(e)) => {
                warn!("tmux command failed: {}", e);
                Err(TmuxError::CommandFailed {
                    command: format!("tmux {}", args.join(" ")),
                    stderr: e.to_string(),
                }
                .into())
            }
            Err(_) => Err(TmuxError::Timeout(self.timeout).into()),
        }
    }

    /// Check if a tmux session exists
    pub async fn session_exists(&self, session_name: &str) -> Result<bool> {
        let result = self.execute(&["has-session", "-t", session_name]).await;
        match result {
            Ok(_) => Ok(true),
            Err(crate::error::Error::Tmux(TmuxError::CommandFailed { .. })) => {
                // "has-session" returns non-zero if session doesn't exist
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    /// Create a new tmux session
    ///
    /// Sets `remain-on-exit on` so that if the command exits or crashes,
    /// the pane stays open showing the exit status rather than disappearing.
    pub async fn create_session(
        &self,
        session_name: &str,
        working_dir: &std::path::Path,
        command: Option<&str>,
    ) -> Result<()> {
        let working_dir_str = working_dir.to_str().unwrap_or(".");

        // Create session with remain-on-exit option so pane stays open if command exits
        let args: Vec<&str> = if let Some(cmd) = command {
            vec![
                "new-session",
                "-d",
                "-s",
                session_name,
                "-c",
                working_dir_str,
                "-x",
                "200",
                "-y",
                "50",
                cmd,
            ]
        } else {
            vec![
                "new-session",
                "-d",
                "-s",
                session_name,
                "-c",
                working_dir_str,
                "-x",
                "200",
                "-y",
                "50",
            ]
        };

        self.execute(&args).await?;

        // Set remain-on-exit so pane stays open if the program exits/crashes
        self.execute(&[
            "set-option",
            "-t",
            session_name,
            "remain-on-exit",
            "on",
        ])
        .await?;

        Ok(())
    }

    /// Kill a tmux session
    pub async fn kill_session(&self, session_name: &str) -> Result<()> {
        self.execute(&["kill-session", "-t", session_name]).await?;
        Ok(())
    }

    /// List all tmux sessions
    pub async fn list_sessions(&self) -> Result<Vec<String>> {
        let output = self
            .execute(&["list-sessions", "-F", "#{session_name}"])
            .await?;

        Ok(output.lines().map(String::from).collect())
    }

    /// Check if a pane is dead (program has exited)
    pub async fn is_pane_dead(&self, session_name: &str) -> Result<bool> {
        let output = self
            .execute(&["list-panes", "-t", session_name, "-F", "#{pane_dead}"])
            .await?;

        // Returns "1" if pane is dead, "0" if alive
        Ok(output.trim() == "1")
    }

    /// Send keys to a tmux session
    pub async fn send_keys(&self, session_name: &str, keys: &str) -> Result<()> {
        self.execute(&["send-keys", "-t", session_name, keys])
            .await?;
        Ok(())
    }

    /// Capture the content of a tmux pane
    pub async fn capture_pane(
        &self,
        session_name: &str,
        start_line: Option<i32>,
        end_line: Option<i32>,
    ) -> Result<String> {
        let mut args = vec!["capture-pane", "-t", session_name, "-p"];

        let start_str;
        let end_str;

        if let Some(start) = start_line {
            start_str = start.to_string();
            args.push("-S");
            args.push(&start_str);
        }

        if let Some(end) = end_line {
            end_str = end.to_string();
            args.push("-E");
            args.push(&end_str);
        }

        self.execute(&args).await
    }
}

impl Default for TmuxExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_executor_creation() {
        let executor = TmuxExecutor::new();
        assert_eq!(executor.timeout, DEFAULT_TIMEOUT);
    }

    #[tokio::test]
    async fn test_executor_with_custom_settings() {
        let executor = TmuxExecutor::with_max_concurrent(8)
            .with_timeout(Duration::from_secs(10));

        assert_eq!(executor.timeout, Duration::from_secs(10));
    }

    // Integration tests would require tmux to be installed
    // They should be marked with #[ignore] and run separately
}
