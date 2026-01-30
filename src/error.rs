//! Error types for claude-commander
//!
//! Uses `thiserror` for ergonomic error definitions with automatic `Display` and `Error` impls.

use std::path::PathBuf;

use thiserror::Error;

use crate::session::SessionId;

/// Top-level error type for claude-commander
#[derive(Error, Debug)]
pub enum Error {
    #[error("Session error: {0}")]
    Session(#[from] SessionError),

    #[error("Tmux error: {0}")]
    Tmux(#[from] TmuxError),

    #[error("Git error: {0}")]
    Git(#[from] GitError),

    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TUI error: {0}")]
    Tui(#[from] TuiError),
}

/// Session management errors
#[derive(Error, Debug)]
pub enum SessionError {
    #[error("Session not found: {0}")]
    NotFound(SessionId),

    #[error("Session already exists: {0}")]
    AlreadyExists(String),

    #[error("Invalid session name '{name}': {reason}")]
    InvalidName { name: String, reason: String },

    #[error("Session {0} is in invalid state for this operation")]
    InvalidState(SessionId),

    #[error("Failed to create session: {0}")]
    CreationFailed(String),

    #[error("Failed to persist session state: {0}")]
    PersistenceFailed(String),

    #[error("Project not found: {0}")]
    ProjectNotFound(String),

    #[error("Maximum sessions reached: {0}")]
    MaxSessionsReached(usize),

    #[error("Tmux session not found: {0} (session may have crashed or been killed)")]
    TmuxSessionNotFound(String),
}

/// Tmux integration errors
#[derive(Error, Debug)]
pub enum TmuxError {
    #[error("Tmux is not installed or not in PATH")]
    NotInstalled,

    #[error("Tmux server not running")]
    ServerNotRunning,

    #[error("Tmux command failed: {command} - {stderr}")]
    CommandFailed { command: String, stderr: String },

    #[error("Failed to capture pane content: {0}")]
    CaptureFailed(String),

    #[error("Session '{0}' not found in tmux")]
    SessionNotFound(String),

    #[error("Tmux command timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("Failed to parse tmux output: {0}")]
    ParseError(String),

    #[error("Semaphore acquire failed")]
    SemaphoreError,

    #[error("PTY error: {0}")]
    PtyError(String),
}

impl From<pty_process::Error> for TmuxError {
    fn from(e: pty_process::Error) -> Self {
        TmuxError::PtyError(e.to_string())
    }
}

impl From<pty_process::Error> for Error {
    fn from(e: pty_process::Error) -> Self {
        Error::Tmux(TmuxError::PtyError(e.to_string()))
    }
}

/// Git operations errors
#[derive(Error, Debug)]
pub enum GitError {
    #[error("Not a git repository: {0}")]
    NotARepository(PathBuf),

    #[error("Git operation failed: {0}")]
    OperationFailed(String),

    #[error("Worktree error: {0}")]
    WorktreeError(String),

    #[error("Branch '{0}' already exists")]
    BranchExists(String),

    #[error("Branch '{0}' not found")]
    BranchNotFound(String),

    #[error("Failed to compute diff: {0}")]
    DiffFailed(String),

    #[error("Gitoxide error: {0}")]
    Gix(String),

    #[error("Invalid reference: {0}")]
    InvalidRef(String),
}

/// Configuration errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to load configuration: {0}")]
    LoadFailed(String),

    #[error("Failed to save configuration: {0}")]
    SaveFailed(String),

    #[error("Invalid configuration value for '{key}': {reason}")]
    InvalidValue { key: String, reason: String },

    #[error("Configuration file not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Failed to create config directory: {0}")]
    DirectoryCreationFailed(PathBuf),
}

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
}

/// Result type alias using our error type
pub type Result<T> = std::result::Result<T, Error>;

/// Convenience trait for converting gitoxide errors
impl From<gix::open::Error> for GitError {
    fn from(e: gix::open::Error) -> Self {
        GitError::Gix(e.to_string())
    }
}

impl From<gix::discover::Error> for GitError {
    fn from(e: gix::discover::Error) -> Self {
        GitError::Gix(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = SessionError::NotFound(SessionId::new());
        assert!(err.to_string().contains("Session not found"));

        let err = TmuxError::NotInstalled;
        assert!(err.to_string().contains("not installed"));

        let err = GitError::NotARepository(PathBuf::from("/tmp/foo"));
        assert!(err.to_string().contains("/tmp/foo"));
    }

    #[test]
    fn test_error_conversion() {
        let session_err = SessionError::NotFound(SessionId::new());
        let _top_err: Error = session_err.into();

        let tmux_err = TmuxError::NotInstalled;
        let _top_err: Error = tmux_err.into();
    }
}
