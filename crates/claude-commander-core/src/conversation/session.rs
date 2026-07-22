//! A long-lived headless `claude` session driven over the stream-json protocol.
//!
//! `claude -p --input-format stream-json --output-format stream-json
//! --include-partial-messages --verbose` keeps a session alive reading
//! newline-delimited user-message JSON from stdin and emitting NDJSON events on
//! stdout. We parse those events into [`ConversationEvent`]s — crucially the
//! incremental `text_delta`s, which arrive as the assistant generates (the
//! supported way to get clean token-level text without scraping a TUI).

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tracing::warn;

use crate::error::TtsError;

// The NDJSON event model and its parsing are shared with the headless Slack
// commander, so they live in [`crate::stream_json`]. Re-exported under the
// historical names so this module's API (and the TTS callers) are unchanged.
pub use crate::stream_json::{StreamEvent as ConversationEvent, parse_event, user_message_line};

/// A running headless conversation session. Dropping it kills the child
/// (`kill_on_drop`), so the session lives exactly as long as this handle.
pub struct ConversationSession {
    child: Child,
    stdin: ChildStdin,
}

impl ConversationSession {
    /// Spawn the streaming `claude` process in `cwd`, forwarding parsed stdout
    /// events on `events`. `command` is the binary to run (default `"claude"`);
    /// `permission_mode` is passed to `--permission-mode` (e.g. `"auto"`) so the
    /// conversation agent can act without interactive approval prompts. When
    /// `resume` is `Some(id)`, `--resume <id>` continues that prior session so
    /// the conversation keeps its history across restarts.
    ///
    /// The child's stderr is captured and logged (not discarded): when `claude`
    /// fails to start a session — a bad flag, an unknown `--resume` id, an auth
    /// problem — it explains itself there, and stdout just closes (surfacing as a
    /// bare `Exited`). Logging it turns an opaque "session ended" into a
    /// diagnosable line in the `conversation` target.
    pub fn spawn(
        command: &str,
        permission_mode: &str,
        cwd: &Path,
        resume: Option<&str>,
        events: mpsc::UnboundedSender<ConversationEvent>,
    ) -> Result<Self, TtsError> {
        let mut cmd = Command::new(command);
        cmd.current_dir(cwd)
            .arg("-p")
            .args(["--input-format", "stream-json"])
            .args(["--output-format", "stream-json"])
            .args(["--permission-mode", permission_mode])
            .arg("--include-partial-messages")
            .arg("--verbose");
        if let Some(id) = resume.filter(|id| !id.is_empty()) {
            cmd.args(["--resume", id]);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| TtsError::Session(format!("failed to start `{command}`: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TtsError::Session("child stdout unavailable".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TtsError::Session("child stdin unavailable".into()))?;

        // Best-effort: log whatever `claude` writes to stderr so startup
        // failures (bad flag, stale `--resume`, auth) aren't lost behind a bare
        // `Exited`. Missing stderr is non-fatal — just skip the logger.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(log_stderr(stderr));
        }

        tokio::spawn(read_events(stdout, events));
        Ok(Self { child, stdin })
    }

    /// Send a user turn to the session.
    pub async fn send_user_message(&mut self, text: &str) -> Result<(), TtsError> {
        self.stdin
            .write_all(user_message_line(text).as_bytes())
            .await
            .map_err(|e| TtsError::Session(format!("write to session failed: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| TtsError::Session(format!("flush to session failed: {e}")))?;
        Ok(())
    }

    /// Best-effort terminate the child now (also happens on drop).
    pub async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
    }
}

async fn read_events(stdout: ChildStdout, events: mpsc::UnboundedSender<ConversationEvent>) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Some(ev) = parse_event(&line)
                    && events.send(ev).is_err()
                {
                    break; // receiver gone
                }
            }
            Ok(None) => break, // EOF
            Err(e) => {
                warn!("conversation session read error: {e}");
                break;
            }
        }
    }
    let _ = events.send(ConversationEvent::Exited);
}

/// Drain the child's stderr, logging each non-empty line under the
/// `conversation` target so a failed launch is diagnosable. Ends when stderr
/// closes (process exit).
async fn log_stderr(stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            warn!(target: "conversation", "claude stderr: {line}");
        }
    }
}
