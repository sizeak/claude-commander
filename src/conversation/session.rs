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

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tracing::warn;

use crate::error::TtsError;

/// Events surfaced from the session's stdout stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationEvent {
    /// Session initialised; carries Claude Code's session id.
    Started { session_id: String },
    /// An incremental chunk of assistant text (a `text_delta`). Chunks can split
    /// mid-word, so consumers must buffer.
    Delta(String),
    /// A new text block started. In an agentic turn the assistant emits text →
    /// tool → more text as separate blocks with no separator between them, so
    /// consumers insert a paragraph break (and flush any pending TTS sentence).
    Break,
    /// The assistant finished the current turn.
    TurnComplete,
    /// A non-fatal error event from the stream.
    Error(String),
    /// The process exited / stdout closed.
    Exited,
}

/// Parse one NDJSON line from the stream into a [`ConversationEvent`], or `None`
/// for events we don't surface (status pings, tool blocks, message framing).
/// Pure and unit-tested.
pub fn parse_event(line: &str) -> Option<ConversationEvent> {
    #[derive(Deserialize)]
    struct Line {
        #[serde(rename = "type")]
        kind: String,
        subtype: Option<String>,
        session_id: Option<String>,
        event: Option<serde_json::Value>,
        is_error: Option<bool>,
    }

    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let parsed: Line = serde_json::from_str(line).ok()?;
    match parsed.kind.as_str() {
        "system" if parsed.subtype.as_deref() == Some("init") => Some(ConversationEvent::Started {
            session_id: parsed.session_id.unwrap_or_default(),
        }),
        "stream_event" => {
            let event = parsed.event?;
            match event.get("type").and_then(|t| t.as_str()) {
                Some("content_block_delta") => {
                    let delta = event.get("delta")?;
                    if delta.get("type").and_then(|t| t.as_str()) == Some("text_delta") {
                        let text = delta
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default();
                        if !text.is_empty() {
                            return Some(ConversationEvent::Delta(text.to_string()));
                        }
                    }
                    None
                }
                // A new *text* block — separates agentic text segments that
                // would otherwise be concatenated with no space between them.
                Some("content_block_start")
                    if event
                        .get("content_block")
                        .and_then(|b| b.get("type"))
                        .and_then(|t| t.as_str())
                        == Some("text") =>
                {
                    Some(ConversationEvent::Break)
                }
                _ => None,
            }
        }
        "result" => {
            if parsed.is_error == Some(true) {
                Some(ConversationEvent::Error(
                    "turn ended with an error".to_string(),
                ))
            } else {
                Some(ConversationEvent::TurnComplete)
            }
        }
        _ => None,
    }
}

/// Serialize a user turn into the stream-json input line (including newline).
pub fn user_message_line(text: &str) -> String {
    let msg = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    });
    let mut line = msg.to_string();
    line.push('\n');
    line
}

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
    /// conversation agent can act without interactive approval prompts.
    pub fn spawn(
        command: &str,
        permission_mode: &str,
        cwd: &Path,
        events: mpsc::UnboundedSender<ConversationEvent>,
    ) -> Result<Self, TtsError> {
        let mut child = Command::new(command)
            .current_dir(cwd)
            .arg("-p")
            .args(["--input-format", "stream-json"])
            .args(["--output-format", "stream-json"])
            .args(["--permission-mode", permission_mode])
            .arg("--include-partial-messages")
            .arg("--verbose")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_event() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc-123","model":"x"}"#;
        assert_eq!(
            parse_event(line),
            Some(ConversationEvent::Started {
                session_id: "abc-123".to_string()
            })
        );
    }

    #[test]
    fn parses_text_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}}"#;
        assert_eq!(
            parse_event(line),
            Some(ConversationEvent::Delta("Hello".to_string()))
        );
    }

    #[test]
    fn text_block_start_is_a_break_others_are_not() {
        let text = r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}"#;
        assert_eq!(parse_event(text), Some(ConversationEvent::Break));
        let thinking = r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"thinking"}}}"#;
        assert_eq!(parse_event(thinking), None);
        let tool = r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"Bash"}}}"#;
        assert_eq!(parse_event(tool), None);
    }

    #[test]
    fn ignores_non_text_deltas_and_framing() {
        // input_json_delta (tool args) is not surfaced
        let tool = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{"}}}"#;
        assert_eq!(parse_event(tool), None);
        // message framing events
        let start = r#"{"type":"stream_event","event":{"type":"message_start"}}"#;
        assert_eq!(parse_event(start), None);
        // status pings
        let status = r#"{"type":"system","subtype":"status","status":"requesting"}"#;
        assert_eq!(parse_event(status), None);
        // empty text delta → None (nothing to say)
        let empty = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":""}}}"#;
        assert_eq!(parse_event(empty), None);
    }

    #[test]
    fn parses_result_as_turn_complete_or_error() {
        assert_eq!(
            parse_event(r#"{"type":"result","subtype":"success","is_error":false}"#),
            Some(ConversationEvent::TurnComplete)
        );
        assert!(matches!(
            parse_event(r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#),
            Some(ConversationEvent::Error(_))
        ));
    }

    #[test]
    fn ignores_garbage_and_blank() {
        assert_eq!(parse_event(""), None);
        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_event(r#"{"type":"assistant","message":{}}"#), None);
    }

    #[test]
    fn user_message_line_shape() {
        let line = user_message_line("hi there");
        assert!(line.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], "hi there");
    }
}
