//! SSH-backed implementations of [`TmuxExec`](crate::tmux::TmuxExec) and
//! [`GitOps`](crate::git::GitOps).
//!
//! Connection management is handled by [`SshSessionPool`], which keeps one
//! `openssh::Session` per `RemoteTransport::connection_key()`. ControlMaster
//! multiplexing under the hood means that all subsequent `session.command(...)`
//! calls reuse the same TCP connection, paying ~1 RTT per command instead of
//! re-handshaking.

mod codespace;
mod git;
mod pool;
mod runner;
mod tmux;

pub use codespace::{
    CodespaceInfo, CodespaceState, gh, gh_codespace_create, gh_codespace_list, gh_codespace_view,
    gh_codespace_wake,
};
pub use git::SshGitOps;
pub use pool::SshSessionPool;
pub use runner::{GhCodespaceRunner, OpensshRunner, RemoteOutput, RemoteRunner};
pub use tmux::SshTmuxExec;
