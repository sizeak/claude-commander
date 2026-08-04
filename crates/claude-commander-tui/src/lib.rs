//! Ratatui terminal frontend for Claude Commander.
//!
//! Event-driven TUI with:
//! - Hierarchical session list (projects + worktrees), sections, and a board view
//! - Preview pane with cached content
//! - Full-screen review-diff view with syntax highlighting and comments
//! - Modal overlays for input and confirmation
//!
//! # Relationship to `claude-commander-core`
//!
//! This crate renders and dispatches; it owns no feature logic. Everything it
//! shows comes from [`claude_commander_core::api::CommanderService`] via the
//! [`CommanderBackend`](claude_commander_core::backend::CommanderBackend) trait,
//! so a session may be local or on a remote server without this crate knowing.
//! Anything worth testing without a terminal belongs in core.
//!
//! The dependency points one way only: core never references this crate. That is
//! what lets the headless `claude-commander-server`, `claude-commander-remote`
//! and the Flutter client's cdylib take core without compiling ratatui at all.

pub mod error;
pub mod hotkey;
pub mod picker;
pub mod theme;

mod app;
mod digit_accumulator;
mod event;
pub(crate) mod list_nav;
mod path_completer;
mod prefs;
mod syntax_highlight;
mod widgets;

/// Visual-regression snapshots of rendered widgets (insta + ratatui's
/// `TestBackend`). Board rendering is snapshotted in `widgets::board::render`
/// instead, next to the fixture harness its unit tests already use.
#[cfg(test)]
mod render_tests;

pub use app::*;
pub use error::{Result, TuiError};
pub use event::*;
pub use theme::Theme;
