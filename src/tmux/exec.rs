//! `TmuxExec` trait — abstraction over how tmux commands are dispatched.
//!
//! Concrete impls live in sibling modules:
//! - `LocalTmuxExec` (`executor.rs`) — spawns local `tmux` via `tokio::process::Command`.
//! - `SshTmuxExec` (`ssh.rs`) — runs `tmux` over a persistent `openssh::Session`.
//!
//! Each backend implements `execute` (single command) and optionally overrides
//! `execute_batch` (multiple commands in one round-trip). All other methods
//! have default impls built on those two primitives, so adding a new backend
//! costs ~20 lines.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tracing::warn;

use crate::error::{Result, TmuxError};

/// Default maximum concurrent tmux commands.
pub const DEFAULT_MAX_CONCURRENT: usize = 16;

/// Default per-command timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Trait abstracting tmux command dispatch.
///
/// Implementors only need to provide `execute`. The default `execute_batch`
/// runs commands sequentially; backends that can do better (e.g. SSH) should
/// override it. Convenience methods are default impls built on top.
#[async_trait]
pub trait TmuxExec: Send + Sync {
    /// Run a single tmux command and return its stdout.
    async fn execute(&self, args: &[&str]) -> Result<String>;

    /// Run multiple tmux commands.
    ///
    /// Default impl runs them sequentially and short-circuits on first error.
    /// Network-bound backends (SSH) should override to dispatch the whole
    /// batch in one round-trip via shell separation.
    async fn execute_batch(&self, commands: &[&[&str]]) -> Result<Vec<String>> {
        let mut out = Vec::with_capacity(commands.len());
        for cmd in commands {
            out.push(self.execute(cmd).await?);
        }
        Ok(out)
    }

    /// Check if tmux is installed and accessible.
    async fn check_installed(&self) -> Result<()> {
        self.execute(&["-V"])
            .await
            .map(|_| ())
            .map_err(|_| TmuxError::NotInstalled.into())
    }

    /// Check if a tmux session exists.
    async fn session_exists(&self, session_name: &str) -> Result<bool> {
        match self.execute(&["has-session", "-t", session_name]).await {
            Ok(_) => Ok(true),
            Err(crate::error::Error::Tmux(TmuxError::CommandFailed { .. })) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Create a new tmux session in the given working directory, optionally
    /// running `command` in the initial pane.
    ///
    /// Sets `remain-on-exit on` so the pane stays open if the command exits,
    /// `mouse on` so scroll wheel enters copy mode, and `history-limit 50000`
    /// for long Claude sessions.
    async fn create_session(
        &self,
        session_name: &str,
        working_dir: &Path,
        command: Option<&str>,
    ) -> Result<()> {
        let working_dir_str = working_dir.to_str().unwrap_or(".");

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
        self.execute(&["set-option", "-t", session_name, "remain-on-exit", "on"])
            .await?;
        self.execute(&["set-option", "-t", session_name, "mouse", "on"])
            .await?;
        self.execute(&["set-option", "-t", session_name, "history-limit", "50000"])
            .await?;
        Ok(())
    }

    /// Kill a tmux session.
    async fn kill_session(&self, session_name: &str) -> Result<()> {
        self.execute(&["kill-session", "-t", session_name]).await?;
        Ok(())
    }

    /// List all tmux sessions by name.
    async fn list_sessions(&self) -> Result<Vec<String>> {
        let output = self
            .execute(&["list-sessions", "-F", "#{session_name}"])
            .await?;
        Ok(output.lines().map(String::from).collect())
    }

    /// Check if a pane has exited.
    async fn is_pane_dead(&self, session_name: &str) -> Result<bool> {
        let output = self
            .execute(&["list-panes", "-t", session_name, "-F", "#{pane_dead}"])
            .await?;
        Ok(output.trim() == "1")
    }

    /// Send keys to a tmux session.
    async fn send_keys(&self, session_name: &str, keys: &str) -> Result<()> {
        self.execute(&["send-keys", "-t", session_name, keys])
            .await?;
        Ok(())
    }

    /// Configure the status bar for a CC tmux session. Errors are logged but
    /// not propagated.
    async fn configure_status_bar(&self, session_name: &str, info: &StatusBarInfo) {
        let left = info.format_left();
        let right = info.format_right();

        let options: &[(&str, &str)] = &[
            ("status-style", &info.status_style),
            ("status-left", &left),
            ("status-left-length", "80"),
            ("status-right", &right),
            ("window-status-format", ""),
            ("window-status-current-format", ""),
        ];

        for (key, value) in options {
            if let Err(e) = self
                .execute(&["set-option", "-t", session_name, key, value])
                .await
            {
                warn!(
                    "Failed to set tmux {} for session {}: {}",
                    key, session_name, e
                );
            }
        }
    }

    /// Capture the content of a tmux pane, with optional scrollback range.
    async fn capture_pane(
        &self,
        session_name: &str,
        start_line: Option<i32>,
        end_line: Option<i32>,
    ) -> Result<String> {
        let mut args = vec!["capture-pane", "-t", session_name, "-p", "-e"];

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

    /// Get the current pane title.
    async fn pane_title(&self, session_name: &str) -> Result<String> {
        self.execute(&["display-message", "-t", session_name, "-p", "#{pane_title}"])
            .await
    }

    /// Capture only the visible pane (no scrollback, no escapes), suitable for
    /// cheap polling-style state probes.
    async fn capture_pane_visible(&self, session_name: &str) -> Result<String> {
        self.execute(&["capture-pane", "-t", session_name, "-p"])
            .await
    }

    /// Probe `(pane_title, visible_pane_content)` for agent-state detection.
    ///
    /// Default uses `execute_batch` so SSH backends collapse the two calls
    /// into one round-trip; local just runs them sequentially.
    async fn agent_probe(&self, session_name: &str) -> Result<(String, String)> {
        let cmds: &[&[&str]] = &[
            &["display-message", "-t", session_name, "-p", "#{pane_title}"],
            &["capture-pane", "-t", session_name, "-p"],
        ];
        let mut results = self.execute_batch(cmds).await?;
        // execute_batch invariant: one entry per command, in order.
        let content = results
            .pop()
            .expect("execute_batch returned fewer results than commands");
        let title = results
            .pop()
            .expect("execute_batch returned fewer results than commands");
        Ok((title, content))
    }
}

/// Info used to render a per-session tmux status bar.
#[derive(Debug, Clone)]
pub struct StatusBarInfo {
    /// Branch name for this session.
    pub branch: String,
    /// GitHub PR number, if one exists.
    pub pr_number: Option<u32>,
    /// Whether the PR has been merged.
    pub pr_merged: bool,
    /// tmux status-style value (e.g. "bg=colour236,fg=colour252").
    pub status_style: String,
    /// Whether this status bar is for a shell session (changes Ctrl-\ hint).
    pub is_shell: bool,
    /// Project name (shown as a prefix).
    pub project_name: String,
}

impl StatusBarInfo {
    /// Format the left side of the status bar.
    ///
    /// Agent session: `project | branch | PR #N | Ctrl-q: detach | Ctrl-\: shell`
    /// Shell session: `project | branch | PR #N | Ctrl-q: detach | Ctrl-\: agent`
    /// `#` is escaped to `##` for tmux format safety.
    pub fn format_left(&self) -> String {
        let safe_branch = self.branch.replace('#', "##");
        let pr = match self.pr_number {
            Some(n) if self.pr_merged => format!(" | PR ##{} merged", n),
            Some(n) => format!(" | PR ##{}", n),
            None => String::new(),
        };
        let toggle_hint = if self.is_shell { "agent" } else { "shell" };
        format!(
            " {} | {}{} | Ctrl-q: detach | Ctrl-\\: {} ",
            self.project_name, safe_branch, pr, toggle_hint
        )
    }

    /// Format the right side of the status bar (currently empty).
    pub fn format_right(&self) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_info(branch: &str, pr_number: Option<u32>, pr_merged: bool) -> StatusBarInfo {
        StatusBarInfo {
            branch: branch.to_string(),
            pr_number,
            pr_merged,
            status_style: "bg=colour236,fg=colour252".to_string(),
            is_shell: false,
            project_name: "my-project".to_string(),
        }
    }

    #[test]
    fn test_status_bar_format_left_basic() {
        let info = test_info("feature-auth", None, false);
        assert_eq!(
            info.format_left(),
            " my-project | feature-auth | Ctrl-q: detach | Ctrl-\\: shell "
        );
    }

    #[test]
    fn test_status_bar_format_left_escapes_hash() {
        let info = test_info("fix-#123-bug", None, false);
        assert!(info.format_left().contains("fix-##123-bug"));
    }

    #[test]
    fn test_status_bar_format_left_open_pr() {
        let info = test_info("feature", Some(42), false);
        assert_eq!(
            info.format_left(),
            " my-project | feature | PR ##42 | Ctrl-q: detach | Ctrl-\\: shell "
        );
    }

    #[test]
    fn test_status_bar_format_left_merged_pr() {
        let info = test_info("feature", Some(42), true);
        assert_eq!(
            info.format_left(),
            " my-project | feature | PR ##42 merged | Ctrl-q: detach | Ctrl-\\: shell "
        );
    }

    #[test]
    fn test_status_bar_format_left_shell_session() {
        let mut info = test_info("feature-auth", None, false);
        info.is_shell = true;
        assert_eq!(
            info.format_left(),
            " my-project | feature-auth | Ctrl-q: detach | Ctrl-\\: agent "
        );
    }

    #[test]
    fn test_status_bar_format_right_empty() {
        let info = test_info("main", None, false);
        assert_eq!(info.format_right(), "");
    }
}
