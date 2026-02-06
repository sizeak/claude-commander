//! Terminal UI module using ratatui
//!
//! Event-driven TUI with:
//! - Hierarchical session list (projects + worktrees)
//! - Preview pane with cached content
//! - Diff pane with syntax highlighting
//! - Modal overlays for input and confirmation

mod app;
mod event;
pub mod theme;
mod widgets;

pub use app::*;
pub use event::*;
pub use theme::Theme;
