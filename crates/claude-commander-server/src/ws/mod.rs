//! WebSocket interactive-terminal endpoint.
//!
//! - [`protocol`] — control-message enums (JSON text frames) + the raw-bytes
//!   (binary frames) framing rules.
//! - [`attach`] — the `/ws/attach` upgrade handler bridging socket frames to a
//!   tmux attach over core's shared `HeadlessAttach` bridge.

pub mod attach;
pub mod protocol;

pub use attach::attach;
