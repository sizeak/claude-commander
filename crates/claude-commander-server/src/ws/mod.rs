//! WebSocket interactive-terminal endpoint.
//!
//! - [`attach`] — the `/ws/attach` upgrade handler bridging socket frames to a
//!   tmux attach over core's shared `HeadlessAttach` bridge. The control-message
//!   enums, framing rules and heartbeat constants it speaks all come straight
//!   from [`claude_commander_protocol::ws`], so the server and every client agree
//!   on the wire contract by construction.

pub mod attach;

pub use attach::attach;
