//! WebSocket terminal protocol — re-exported from the shared
//! [`claude_commander_protocol`] crate.
//!
//! The control-message enums and the binary/text framing discipline are defined
//! once in the protocol crate so the server and every client agree on the wire
//! shape by construction. This module re-exports them so the existing
//! `super::protocol::{...}` import paths in [`super::attach`] keep working.

pub use claude_commander_protocol::ws::{AttachKind, ClientControl, DetachReason, ServerControl};
