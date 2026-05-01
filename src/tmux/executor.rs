//! Local tmux command executor.
//!
//! Spawns `tmux` via `tokio::process::Command` with a semaphore for
//! concurrency control and a per-command timeout. Implements the
//! [`TmuxExec`](super::TmuxExec) trait; convenience methods (`session_exists`,
//! `create_session`, etc.) come from default trait impls.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, instrument, warn};

use super::exec::{DEFAULT_MAX_CONCURRENT, DEFAULT_TIMEOUT, TmuxExec};
use crate::error::{Result, TmuxError};

/// Local-process tmux executor.
///
/// Uses a semaphore to limit concurrent tmux invocations, preventing resource
/// exhaustion when managing many sessions.
#[derive(Clone)]
pub struct LocalTmuxExec {
    semaphore: Arc<Semaphore>,
    timeout: Duration,
}

impl LocalTmuxExec {
    /// Create a new executor with default settings.
    pub fn new() -> Self {
        Self::with_max_concurrent(DEFAULT_MAX_CONCURRENT)
    }

    /// Create an executor with a custom concurrency limit.
    pub fn with_max_concurrent(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Set the per-command timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait]
impl TmuxExec for LocalTmuxExec {
    #[instrument(skip(self), fields(args = ?args))]
    async fn execute(&self, args: &[&str]) -> Result<String> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| TmuxError::SemaphoreError)?;

        let mut cmd = Command::new("tmux");
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

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

    /// Local impl: special-case `tmux -V` to map spawn failure to
    /// `NotInstalled`. Avoids relying on `execute`'s error mapping which
    /// loses the "binary missing" distinction.
    async fn check_installed(&self) -> Result<()> {
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
}

impl Default for LocalTmuxExec {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_executor_creation() {
        let executor = LocalTmuxExec::new();
        assert_eq!(executor.timeout, DEFAULT_TIMEOUT);
    }

    #[tokio::test]
    async fn test_executor_with_custom_settings() {
        let executor = LocalTmuxExec::with_max_concurrent(8).with_timeout(Duration::from_secs(10));
        assert_eq!(executor.timeout, Duration::from_secs(10));
    }
}
