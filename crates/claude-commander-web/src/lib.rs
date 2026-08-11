//! `claude-commander-web` — a standalone web UI for Claude Commander.
//!
//! It serves the browser SPA and reverse-proxies `/api` + `/ws/attach` to a
//! running [`claude-commander-server`]. It is a pure *client* of that server —
//! it never links `claude-commander-core` (no tmux/gix), so it stays small and
//! portable. See [`config::AuthMode`] for the BFF vs. pass-through auth modes.

pub mod assets;
pub mod auth;
pub mod config;
pub mod proxy;
pub mod router;
pub mod ws_proxy;

pub use config::{AppState, AuthMode};
pub use router::build_router;
