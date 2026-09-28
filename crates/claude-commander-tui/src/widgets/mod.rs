//! TUI widgets
//!
//! Custom ratatui widgets for the application:
//! - `board` - Kanban board navigation state, layout geometry, and widget
//! - `InfoView` - Session info, PR details, AI summary (Info modal)
//! - `Preview` - scrollable ANSI pane capture for the list views' right pane
//! - `tree_list` - session-list widget for the list views
//! - `status_glyph` / `pr_colors` - row-rendering helpers shared by both

pub mod board;
mod info_view;
pub(crate) mod pr_colors;
mod preview;
pub mod status_glyph;
pub mod tree_list;

pub use info_view::*;
pub use preview::{Preview, PreviewState};
pub use tree_list::{TreeList, TreeListState, list_has_mixed_programs, worktree_display_info};
