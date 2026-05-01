//! SSH-backed [`TmuxExec`](crate::tmux::TmuxExec) implementation.
//!
//! Dispatches tmux commands through a [`RemoteRunner`] (openssh-pooled
//! for `Ssh` transport, `gh codespace ssh` per call for `Codespace`).
//! Overrides `execute_batch` to fold multiple commands into one remote
//! invocation via a shell pipeline with a unique delimiter; default trait
//! methods that route through `execute_batch` (e.g. `agent_probe`) get the
//! batching for free.

use std::sync::Arc;

use async_trait::async_trait;
use shell_escape::unix::escape;
use tokio::sync::Semaphore;
use tracing::{debug, instrument, warn};

use super::runner::RemoteRunner;
use crate::error::{Result, TmuxError};
use crate::tmux::TmuxExec;

/// SSH-backed tmux executor.
pub struct SshTmuxExec {
    runner: Arc<dyn RemoteRunner>,
    semaphore: Arc<Semaphore>,
}

impl SshTmuxExec {
    pub fn new(runner: Arc<dyn RemoteRunner>, max_concurrent: usize) -> Self {
        Self {
            runner,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    fn shell_quote(arg: &str) -> String {
        escape(arg.into()).into_owned()
    }
}

#[async_trait]
impl TmuxExec for SshTmuxExec {
    #[instrument(skip(self), fields(args = ?args))]
    async fn execute(&self, args: &[&str]) -> Result<String> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| TmuxError::SemaphoreError)?;

        let mut argv: Vec<&str> = vec!["tmux"];
        argv.extend_from_slice(args);
        let output = self.runner.run(&argv).await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(TmuxError::CommandFailed {
                command: format!("ssh tmux {}", args.join(" ")),
                stderr,
            }
            .into())
        }
    }

    /// Run multiple tmux commands in one remote invocation.
    ///
    /// Builds `sh -c "tmux ... ; printf %s\\n DELIM ; tmux ... ; ..."` and
    /// splits the captured stdout on the delimiter. The delimiter is a fresh
    /// UUID per call so it can't collide with anything tmux emits.
    async fn execute_batch(&self, commands: &[&[&str]]) -> Result<Vec<String>> {
        if commands.is_empty() {
            return Ok(Vec::new());
        }
        if commands.len() == 1 {
            return Ok(vec![self.execute(commands[0]).await?]);
        }

        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| TmuxError::SemaphoreError)?;

        let delim = format!("---CC-{}-DELIM---", uuid::Uuid::new_v4().simple());
        let mut script = String::new();
        for (i, args) in commands.iter().enumerate() {
            if i > 0 {
                script.push_str(&format!(
                    "printf '\\n%s\\n' {} ; ",
                    Self::shell_quote(&delim)
                ));
            }
            script.push_str("tmux");
            for arg in args.iter() {
                script.push(' ');
                script.push_str(&Self::shell_quote(arg));
            }
            script.push_str(" ; ");
        }
        debug!("ssh batch script: {}", script);

        let output = self.runner.run(&["sh", "-c", &script]).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("ssh tmux batch returned non-zero: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<String> = stdout
            .split(&format!("\n{}\n", delim))
            .map(|s| s.to_string())
            .collect();

        if parts.len() != commands.len() {
            return Err(TmuxError::CommandFailed {
                command: "ssh tmux batch".to_string(),
                stderr: format!("expected {} parts, got {}", commands.len(), parts.len()),
            }
            .into());
        }

        Ok(parts)
    }

    async fn check_installed(&self) -> Result<()> {
        self.execute(&["-V"])
            .await
            .map(|_| ())
            .map_err(|_| TmuxError::NotInstalled.into())
    }
}
