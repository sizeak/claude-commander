//! Pure Rust git operations using gitoxide
//!
//! Provides git functionality without any CLI dependencies:
//! - `GitBackend` - Core gitoxide operations
//! - `WorktreeManager` - Worktree lifecycle management
//! - `DiffCache` - Cached diff computation

mod backend;
mod diff;
mod pr;
mod worktree;

pub use backend::*;
pub use diff::*;
pub use pr::*;
pub use worktree::*;
