//! This crate's error type.
//!
//! Per CLAUDE.md, a crate leaving core brings its own error type rather than
//! extending core's hierarchy — `TuiError` used to be a variant of
//! [`claude_commander_core::Error`], which meant the headless server's error
//! enum carried a "failed to initialize terminal" case it could never produce.
//!
//! [`TuiError::Core`] deliberately has **no** `#[from]`. Terminal failures and
//! library failures stay distinguishable, and because there is no blanket
//! conversion, every point where a core error becomes a TUI error has to be
//! written out. There are exactly two ([`crate::picker::run_session_picker`]
//! loading `AppState`, and `App::run` propagating a failed tmux probe), which is
//! few enough that making them explicit costs nothing and documents the seam.
//!
//! Functions whose failures are *only* core's keep returning
//! [`claude_commander_core::Result`] unchanged — see `prefs::persist` and
//! `app::actions::load_branch_entries`. Wrapping those would add noise without
//! adding information.

use thiserror::Error;

/// TUI-related errors
#[derive(Error, Debug)]
pub enum TuiError {
    #[error("Failed to initialize terminal: {0}")]
    InitFailed(String),

    #[error("Failed to restore terminal: {0}")]
    RestoreFailed(String),

    #[error("Render error: {0}")]
    RenderError(String),

    #[error("Event handling error: {0}")]
    EventError(String),

    /// A core operation failed on a code path that also produces terminal
    /// errors. Transparent, so the underlying error's own message and
    /// [`source`](std::error::Error::source) chain survive the crossing — but
    /// with no `#[from]`, so the conversion is always spelled out at the call
    /// site (`.map_err(TuiError::Core)`).
    #[error(transparent)]
    Core(claude_commander_core::Error),
}

/// Result type alias for this crate.
pub type Result<T> = std::result::Result<T, TuiError>;
