//! Configuration and persistence module
//!
//! Handles:
//! - User configuration (`~/.claude-commander/config.toml`)
//! - Persistent state (`~/.claude-commander/state.json`)
//! - Worktree directory management

mod settings;
pub mod storage;
mod store;

pub use settings::*;
pub use storage::*;
pub use store::StateStore;
