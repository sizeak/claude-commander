//! Pure Rust git operations using gitoxide
//!
//! Provides git functionality without any CLI dependencies:
//! - `GitBackend` - Core gitoxide operations
//! - `WorktreeManager` - Worktree lifecycle management
//! - `DiffCache` - Cached diff computation

mod backend;
mod diff;
mod ops;
mod pr;
mod summary;
mod worktree;
mod worktree_include;

pub use backend::*;
pub(crate) use diff::parse_diff_stat;
pub use diff::*;
pub use ops::*;
pub use pr::*;
pub use summary::*;
pub(crate) use worktree::parse_worktree_list;
pub use worktree::*;
