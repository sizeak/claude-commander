//! Session management module
//!
//! Provides the hierarchical session model:
//! - `Project` - A git repository (parent)
//! - `WorktreeSession` - A worktree session within a project (child)
//! - `SessionManager` - Coordinates session lifecycle

mod manager;
mod types;

pub use manager::*;
pub use types::*;
