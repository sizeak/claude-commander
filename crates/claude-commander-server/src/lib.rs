//! `claude-commander-server` library surface.
//!
//! The server is shipped as a binary (`src/main.rs`), but its modules live here
//! so integration tests under `tests/` can build a router in-process and drive
//! it with a real client. `main.rs` is a thin wrapper that resolves config +
//! auth and serves [`build_router`] (matching the project's "keep main thin;
//! logic in lib for testability" rule).

pub mod auth;
pub mod config;
pub mod error;
pub mod handlers;
pub mod router;
pub mod slack;
pub mod state;
pub mod ws;

pub use auth::AuthConfig;
pub use config::ServerConfig;
pub use router::build_router;
pub use state::AppState;
