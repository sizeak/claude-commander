//! Configuration and persistence module
//!
//! Handles:
//! - User configuration (`~/.claude-commander/config.toml`)
//! - Persistent state (`~/.claude-commander/state.json`)
//! - Worktree directory management

mod settings;
mod storage;

pub use settings::*;
pub use storage::*;
