//! Shared wire contract for the `claude-commander-server` HTTP + WebSocket API.
//!
//! This crate is the single source of truth for the types that cross the
//! network boundary, so the server and every client (the mobile/desktop app, a
//! browser, the CLI) agree on the serde shape *by construction* rather than by
//! hand-maintained mirrors.
//!
//! It is deliberately dependency-light — `serde` + `serde_json` only, no tmux,
//! git, or filesystem code — so it cross-compiles cleanly to mobile targets
//! (Android/iOS) where the heavyweight `claude-commander-core` crate cannot go.
//!
//! Every type here derives both `Serialize` and `Deserialize`: the server
//! serializes, clients deserialize (and vice-versa for request bodies and the
//! WebSocket control frames).

pub mod api;
pub mod comment;
pub mod diff;
pub mod pr;
pub mod session;
pub mod ws;
