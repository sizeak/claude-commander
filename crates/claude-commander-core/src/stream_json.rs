//! Shared parsing for the headless `claude` stream-json protocol.
//!
//! `claude -p --input-format stream-json --output-format stream-json
//! --include-partial-messages --verbose` reads newline-delimited user-message
//! JSON on stdin and emits NDJSON events on stdout. Two independent features
//! drive that same subprocess — the TTS [`conversation`](crate::conversation)
//! overlay and the headless Slack [`commander`](crate::commander) — so the pure,
//! side-effect-free parsing of those events lives here, unit-tested once, rather
//! than being duplicated (or coupled to the TTS module's error type).

use serde::Deserialize;

/// Events surfaced from a stream-json session's stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
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

/// Parse one NDJSON line from the stream into a [`StreamEvent`], or `None`
/// for events we don't surface (status pings, tool blocks, message framing).
/// Pure and unit-tested.
pub fn parse_event(line: &str) -> Option<StreamEvent> {
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
        "system" if parsed.subtype.as_deref() == Some("init") => Some(StreamEvent::Started {
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
                            return Some(StreamEvent::Delta(text.to_string()));
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
                    Some(StreamEvent::Break)
                }
                _ => None,
            }
        }
        "result" => {
            if parsed.is_error == Some(true) {
                Some(StreamEvent::Error("turn ended with an error".to_string()))
            } else {
                Some(StreamEvent::TurnComplete)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_event() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc-123","model":"x"}"#;
        assert_eq!(
            parse_event(line),
            Some(StreamEvent::Started {
                session_id: "abc-123".to_string()
            })
        );
    }

    #[test]
    fn parses_text_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}}"#;
        assert_eq!(
            parse_event(line),
            Some(StreamEvent::Delta("Hello".to_string()))
        );
    }

    #[test]
    fn text_block_start_is_a_break_others_are_not() {
        let text = r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}"#;
        assert_eq!(parse_event(text), Some(StreamEvent::Break));
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
            Some(StreamEvent::TurnComplete)
        );
        assert!(matches!(
            parse_event(r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#),
            Some(StreamEvent::Error(_))
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
