//! Session management module
//!
//! Provides the hierarchical session model:
//! - `Project` - A git repository (parent)
//! - `WorktreeSession` - A worktree session within a project (child)

mod types;

pub use types::*;
