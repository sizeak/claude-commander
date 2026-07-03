//! WebSocket terminal protocol: framing + control-message types.
//!
//! Two kinds of frame travel over the `/ws/attach` socket, and the split is
//! deliberate:
//!
//! - **Raw PTY bytes use WebSocket *binary* frames.** Terminal output and
//!   keystrokes are arbitrary byte streams — escape sequences and partial
//!   multibyte UTF-8 are routine — so routing them through *text* frames would
//!   corrupt them (text frames must be valid UTF-8). The bridge never sees JSON;
//!   it sees bytes.
//! - **Control messages use WebSocket *text* frames carrying JSON.** These are
//!   small, structured, and human-debuggable: the handshake (`auth`, `attach`),
//!   out-of-band resize, explicit detach, and the server's replies.
//!
//! The server discriminates purely on frame *kind*: a binary frame is always
//! PTY data, a text frame is always a control message. There is no in-band
//! tagging mixing the two, so the discipline can't be violated by a malformed
//! payload.

use serde::{Deserialize, Serialize};

/// A control message sent by the *client* (browser/native UI) as a JSON text
/// frame. The `auth` then `attach` messages form the mandatory handshake;
/// `resize` and `detach` are valid in steady state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientControl {
    /// First frame: authenticate the socket. Browsers can't set headers on the
    /// WS upgrade, so the token travels in-band here. **Never logged.**
    Auth { token: String },
    /// Second frame: attach to a session. `session_id` is resolved exactly like
    /// the HTTP API's `find_session` (full UUID, ID prefix, or exact title).
    /// `kind` selects the agent pane (default) or the paired shell pane; it is
    /// omitted on the wire for an agent attach, so an old client's
    /// `{"type":"attach","session_id":…}` frame parses unchanged.
    Attach {
        session_id: String,
        #[serde(default, skip_serializing_if = "AttachKind::is_agent")]
        kind: AttachKind,
    },
    /// Resize the remote PTY. Sent whenever the client's terminal viewport
    /// changes.
    Resize { cols: u16, rows: u16 },
    /// Explicitly detach: kill the `tmux attach-session` child but leave the
    /// tmux session (and the program inside it) running.
    Detach,
}

/// A control message sent by the *server* as a JSON text frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerControl {
    /// Handshake succeeded and the bridge is attached. Echoes the resolved tmux
    /// session name so the client can label the terminal.
    Ready { session: String },
    /// The attach ended. `reason` distinguishes a client-requested detach from a
    /// session that ended or a transport error, for client-side UX.
    Detached { reason: DetachReason },
    /// A handshake or steady-state error. `message` is safe to surface to the
    /// user; it never contains the auth token.
    Error { message: String },
}

/// Fixed [`ServerControl::Error`] handshake message for a rejected auth token.
/// Pinned as a constant so the server's wording and the client's error
/// classifier reference the same string and can't drift out of sync.
pub const WS_ERR_AUTH: &str = "authentication failed";

/// Fixed [`ServerControl::Error`] handshake message for an attach to a session
/// that doesn't exist. Shared by the server (which sends it) and the client
/// (which classifies it), so the wording is a single source of truth.
pub const WS_ERR_NO_SESSION: &str = "no such session";

/// Which pane of a session to attach to. Mirrors core's `backend::AttachKind`
/// but lives here so the wire shape has one source of truth. Serialized inside
/// [`ClientControl::Attach`]; [`Agent`](Self::Agent) is the default and is
/// omitted on the wire (see the `skip_serializing_if` on the field), so the
/// frame an old client sends is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachKind {
    /// The agent (e.g. Claude) pane — the session's primary tmux session.
    #[default]
    Agent,
    /// The paired shell pane (Ctrl+\ toggles here), created on demand.
    Shell,
}

impl AttachKind {
    /// Whether this is the default agent pane. Used to skip serializing the
    /// field for an agent attach so the wire form matches the pre-`kind` frame.
    pub fn is_agent(&self) -> bool {
        matches!(self, AttachKind::Agent)
    }
}

/// Why an attach ended. Serialized as part of [`ServerControl::Detached`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetachReason {
    /// The client sent a `detach` control frame.
    ClientRequest,
    /// The tmux session ended (the program inside it exited).
    SessionEnded,
    /// The transport dropped (socket closed, heartbeat timed out).
    Transport,
}

impl ClientControl {
    /// Parse a control message from a text-frame payload.
    pub fn from_text(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Render this control message as a JSON string for a text frame.
    pub fn to_text(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl ServerControl {
    /// Parse a control message from a text-frame payload (client side).
    pub fn from_text(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Render this control message as a JSON string for a text frame.
    pub fn to_text(&self) -> String {
        // The variants are simple, infallible-to-serialize structs; fall back to
        // a generic error frame on the (impossible) failure rather than panic.
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"type":"error","message":"failed to serialize control message"}"#.to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_auth_round_trip() {
        let msg = ClientControl::Auth {
            token: "s3cret".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"auth","token":"s3cret"}"#);
        assert_eq!(ClientControl::from_text(&json).unwrap(), msg);
    }

    #[test]
    fn client_attach_round_trip() {
        // An agent attach omits `kind` on the wire (byte-identical to the
        // pre-`kind` frame), so old and new peers agree.
        let msg = ClientControl::Attach {
            session_id: "abc123".into(),
            kind: AttachKind::Agent,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"attach","session_id":"abc123"}"#);
        assert_eq!(ClientControl::from_text(&json).unwrap(), msg);
    }

    #[test]
    fn client_attach_shell_round_trip() {
        // A shell attach carries `kind` explicitly.
        let msg = ClientControl::Attach {
            session_id: "abc123".into(),
            kind: AttachKind::Shell,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"attach","session_id":"abc123","kind":"shell"}"#
        );
        assert_eq!(ClientControl::from_text(&json).unwrap(), msg);
    }

    #[test]
    fn old_attach_frame_without_kind_parses_as_agent() {
        // Backward compatibility: a frame from a client that predates the `kind`
        // field must still parse, defaulting to the agent pane.
        let parsed =
            ClientControl::from_text(r#"{"type":"attach","session_id":"abc123"}"#).unwrap();
        assert_eq!(
            parsed,
            ClientControl::Attach {
                session_id: "abc123".into(),
                kind: AttachKind::Agent,
            }
        );
    }

    #[test]
    fn client_resize_round_trip() {
        let msg = ClientControl::Resize {
            cols: 120,
            rows: 40,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"resize","cols":120,"rows":40}"#);
        assert_eq!(ClientControl::from_text(&json).unwrap(), msg);
    }

    #[test]
    fn client_detach_round_trip() {
        let msg = ClientControl::Detach;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"detach"}"#);
        assert_eq!(ClientControl::from_text(&json).unwrap(), msg);
    }

    #[test]
    fn client_to_text_round_trips() {
        // The client serializes its own control frames to send them; verify the
        // helper matches the canonical wire form.
        let msg = ClientControl::Resize { cols: 80, rows: 24 };
        let text = msg.to_text().unwrap();
        assert_eq!(text, r#"{"type":"resize","cols":80,"rows":24}"#);
        assert_eq!(ClientControl::from_text(&text).unwrap(), msg);
    }

    #[test]
    fn server_ready_round_trip() {
        let msg = ServerControl::Ready {
            session: "cc-1234abcd".into(),
        };
        let json = msg.to_text();
        assert_eq!(json, r#"{"type":"ready","session":"cc-1234abcd"}"#);
        assert_eq!(ServerControl::from_text(&json).unwrap(), msg);
    }

    #[test]
    fn server_detached_round_trip() {
        for (reason, tag) in [
            (DetachReason::ClientRequest, "client_request"),
            (DetachReason::SessionEnded, "session_ended"),
            (DetachReason::Transport, "transport"),
        ] {
            let msg = ServerControl::Detached { reason };
            let json = msg.to_text();
            assert_eq!(json, format!(r#"{{"type":"detached","reason":"{tag}"}}"#));
            assert_eq!(ServerControl::from_text(&json).unwrap(), msg);
        }
    }

    #[test]
    fn server_error_round_trip() {
        let msg = ServerControl::Error {
            message: "no such session".into(),
        };
        let json = msg.to_text();
        assert_eq!(json, r#"{"type":"error","message":"no such session"}"#);
        assert_eq!(ServerControl::from_text(&json).unwrap(), msg);
    }

    #[test]
    fn unknown_control_type_is_rejected() {
        assert!(ClientControl::from_text(r#"{"type":"bogus"}"#).is_err());
        // A binary-only payload (not JSON) must not parse as a control message —
        // the handler relies on this so binary frames are never misread as text.
        assert!(ClientControl::from_text("\x1b[2J not json").is_err());
    }

    #[test]
    fn missing_required_field_is_rejected() {
        // `auth` without a token is invalid.
        assert!(ClientControl::from_text(r#"{"type":"auth"}"#).is_err());
        // `attach` without a session_id is invalid.
        assert!(ClientControl::from_text(r#"{"type":"attach"}"#).is_err());
    }
}
