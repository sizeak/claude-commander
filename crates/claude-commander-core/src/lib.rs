//! Claude Commander - A high-performance terminal UI for managing Claude coding sessions
//!
//! This crate provides an async-first, actor-based architecture for managing
//! multiple AI coding sessions through tmux and git worktrees.
//!
//! # Architecture
//!
//! The application is built around several key actors:
//! - **TUI Actor** - Handles terminal rendering and user input
//! - **SessionManager Actor** - Coordinates session lifecycle
//! - **TmuxActor** - Per-session tmux integration
//! - **GitActor** - Per-session git operations
//!
//! # Modules
//!
//! - [`session`] - Hierarchical session model (Projects and WorktreeSessions)
//! - [`tmux`] - Async tmux integration with caching
//! - [`git`] - Pure Rust git operations via gitoxide
//! - [`tui`] - Event-driven terminal UI with ratatui
//! - [`config`] - Configuration and state persistence
//! - [`error`] - Error types

pub mod agent;
pub mod api;
pub mod backend;
pub mod cli;
pub mod commander;
pub mod comment;
pub mod config;
pub mod conversation;
pub mod error;
pub mod fuzzy;
pub mod git;
pub mod paste_image;
pub mod picker;
pub mod reviewed;
pub mod session;
pub mod telemetry;
pub mod term_caps;
pub mod tmux;
pub mod tui;

pub use config::keybindings::editor_trigger_bytes;
pub use config::{AppState, Config, StateStore};
pub use error::{Error, Result};
pub use session::{
    AgentState, Project, ProjectId, SessionId, SessionListItem, SessionStatus, WorktreeSession,
};
pub use tmux::{AttachResult, attach_to_session};

/// This library's version, i.e. the `claude-commander-core` crate's. Reported
/// as telemetry's `lib_version` and compared against a remote server's build.
/// It is *not* the frontend's version — a frontend (the TUI binary, the mobile
/// client) reports its own; see [`telemetry::FrontendInfo`].
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
