//! `claude-commander-server` library surface.
//!
//! The server has two frontends: the `claude-commander-server` binary
//! (`src/main.rs`) and the TUI, which embeds it in-process and shares its
//! `CommanderService` (see [`embed`]). Both are thin wrappers that resolve
//! config + auth and hand off to [`embed::start`], so the policy worth testing
//! lives here rather than in either `main` — matching the project's "keep main
//! thin; logic in lib for testability" rule. Integration tests under `tests/`
//! build a router in-process and drive it with a real client.
//!
//! The persisted `[server]` table itself is
//! [`claude_commander_core::config::ServerConfig`], not a type of this crate's;
//! [`config`] explains why it lives there.

pub mod auth;
pub mod config;
pub mod embed;
pub mod error;
pub mod extract;
pub mod handlers;
pub mod router;
pub mod state;
pub mod ws;

pub use auth::AuthConfig;
pub use embed::{EmbeddedServer, StartError, TokenDecision};
pub use router::build_router;
pub use state::AppState;
