//! TUI widgets
//!
//! Custom ratatui widgets for the application:
//! - `TreeList` - Hierarchical session list
//! - `Preview` - Pane content preview
//! - `DiffView` - Diff display with syntax highlighting

mod diff_view;
mod preview;
mod tree_list;

pub use diff_view::*;
pub use preview::*;
pub use tree_list::*;
