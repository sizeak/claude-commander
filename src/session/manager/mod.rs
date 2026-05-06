//! Session manager - coordinates session lifecycle
//!
//! Handles the creation, restart, and termination of sessions,
//! coordinating between tmux and git operations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{debug, info, instrument, warn};

use std::collections::HashMap;
use tokio::sync::RwLock as TokioRwLock;

use crate::config::{AppState, ConfigStore, StateStore};
use crate::error::{Result, SessionError};
use crate::git::{DiffCache, DiffInfo, GitBackend, GitOps, LocalGitOps, WorktreeManager};
use crate::remote::{SshGitOps, SshSessionPool, SshTmuxExec};
use crate::session::{
    Project, ProjectId, RemoteTransport, SessionId, SessionStatus, WorktreeSession,
};
use crate::tmux::{CapturedContent, ContentCapture, LocalTmuxExec, StatusBarInfo, TmuxExec};

/// Result of scanning a directory for git repositories
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// Number of new projects added
    pub added: usize,
    /// Number of repos skipped because they already existed
    pub skipped: usize,
}

mod cascade;
mod content;
mod lifecycle;
mod project_shell;
mod projects;
mod share;
mod shell;
mod worktree_sync;

pub use cascade::{CascadeOutcome, PushStackOutcome};
pub use share::{JoinedShareTarget, diagnose_joiner_logs};

#[cfg(test)]
mod tests;

/// Cached `(TmuxExec, GitOps)` pair for one remote host.
#[derive(Clone)]
struct RemoteOps {
    tmux: Arc<dyn TmuxExec>,
    git: Arc<dyn GitOps>,
}

/// Session manager coordinates all session operations
pub struct SessionManager {
    /// Shared configuration store (hot-reloaded)
    config_store: Arc<ConfigStore>,
    /// Concurrent-safe persistent state store
    pub store: Arc<StateStore>,
    /// Local tmux executor (used for local-project sessions).
    pub tmux: Arc<dyn TmuxExec>,
    /// Local git ops (used for local-project sessions).
    pub git_ops: Arc<dyn GitOps>,
    /// Pool of `openssh::Session`s, one per remote host.
    ssh_pool: Arc<SshSessionPool>,
    /// Cached SSH-backed ops per `RemoteTransport::connection_key()`.
    remote_ops: Arc<TokioRwLock<HashMap<String, RemoteOps>>>,
    /// Max concurrent tmux commands (used for SSH executors as well).
    max_concurrent_tmux: usize,
    /// Content capture cache
    content_capture: ContentCapture,
    /// Diff cache for sessions
    diff_cache: DiffCache<SessionId>,
    /// Diff cache for projects
    project_diff_cache: DiffCache<ProjectId>,
    /// Tmux status-style string derived from theme
    tmux_status_style: String,
}

impl Clone for SessionManager {
    fn clone(&self) -> Self {
        Self {
            config_store: self.config_store.clone(),
            store: self.store.clone(),
            tmux: self.tmux.clone(),
            git_ops: self.git_ops.clone(),
            ssh_pool: self.ssh_pool.clone(),
            remote_ops: self.remote_ops.clone(),
            max_concurrent_tmux: self.max_concurrent_tmux,
            content_capture: self.content_capture.clone(),
            diff_cache: self.diff_cache.clone(),
            project_diff_cache: self.project_diff_cache.clone(),
            tmux_status_style: self.tmux_status_style.clone(),
        }
    }
}

impl SessionManager {
    /// Create a new session manager
    ///
    /// Note: `max_concurrent_tmux`, `capture_cache_ttl_ms`, and `diff_cache_ttl_ms`
    /// are read from the config at construction time and are **not** hot-reloaded.
    pub fn new(
        config_store: Arc<ConfigStore>,
        store: Arc<StateStore>,
        tmux_status_style: impl Into<String>,
    ) -> Self {
        let config = config_store.read();
        let tmux: Arc<dyn TmuxExec> = Arc::new(LocalTmuxExec::with_max_concurrent(
            config.max_concurrent_tmux,
        ));
        let content_capture = ContentCapture::with_ttl(std::time::Duration::from_millis(
            config.capture_cache_ttl_ms,
        ));
        let diff_cache =
            DiffCache::with_ttl(std::time::Duration::from_millis(config.diff_cache_ttl_ms));
        let project_diff_cache =
            DiffCache::with_ttl(std::time::Duration::from_millis(config.diff_cache_ttl_ms));
        drop(config);

        let git_ops: Arc<dyn GitOps> = Arc::new(LocalGitOps::new());
        let max_concurrent_tmux = config_store.read().max_concurrent_tmux;
        Self {
            config_store,
            store,
            tmux,
            git_ops,
            ssh_pool: Arc::new(SshSessionPool::new()),
            remote_ops: Arc::new(TokioRwLock::new(HashMap::new())),
            max_concurrent_tmux,
            content_capture,
            diff_cache,
            project_diff_cache,
            tmux_status_style: tmux_status_style.into(),
        }
    }

    /// Check if tmux is available
    pub async fn check_tmux(&self) -> Result<()> {
        self.tmux.check_installed().await
    }

    /// Resolve the [`AttachTarget`] for a project — i.e. how should the
    /// PTY-attach loop dispatch its `tmux attach-session` invocation?
    pub async fn attach_target_for_project(
        &self,
        project_id: &ProjectId,
    ) -> Result<crate::tmux::AttachTarget> {
        let state = self.store.read().await;
        let project = state
            .get_project(project_id)
            .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
        Ok(match &project.remote {
            None => crate::tmux::AttachTarget::Local,
            Some(RemoteTransport::Ssh { host }) => {
                crate::tmux::AttachTarget::Ssh { host: host.clone() }
            }
            Some(RemoteTransport::Codespace { name }) => crate::tmux::AttachTarget::Codespace {
                codespace_name: name.clone(),
            },
        })
    }

    /// Convenience: resolve attach target by session id (looks up the
    /// owning project).
    pub async fn attach_target_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::tmux::AttachTarget> {
        let project_id = {
            let state = self.store.read().await;
            state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?
                .project_id
        };
        self.attach_target_for_project(&project_id).await
    }

    /// Resolve the `(TmuxExec, GitOps)` pair for the project that owns
    /// `project_id`. Local projects route through the local backends; remote
    /// projects open (or reuse) one persistent `openssh::Session` per host.
    ///
    /// Use this whenever a session-management code path needs to dispatch
    /// commands; never use `self.tmux` / `self.git_ops` directly inside
    /// project-scoped flows.
    pub async fn ops_for(
        &self,
        project_id: &ProjectId,
    ) -> Result<(Arc<dyn TmuxExec>, Arc<dyn GitOps>)> {
        let remote = {
            let state = self.store.read().await;
            state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?
                .remote
                .clone()
        };
        match remote {
            None => Ok((Arc::clone(&self.tmux), Arc::clone(&self.git_ops))),
            Some(transport) => self.ops_for_remote(&transport).await,
        }
    }

    /// Build (or fetch from cache) an SSH-backed ops pair for `transport`.
    async fn ops_for_remote(
        &self,
        transport: &RemoteTransport,
    ) -> Result<(Arc<dyn TmuxExec>, Arc<dyn GitOps>)> {
        let key = transport.connection_key();
        {
            let guard = self.remote_ops.read().await;
            if let Some(ops) = guard.get(&key) {
                return Ok((Arc::clone(&ops.tmux), Arc::clone(&ops.git)));
            }
        }
        let runner = self.ssh_pool.get_or_connect(transport).await?;
        let tmux: Arc<dyn TmuxExec> = Arc::new(SshTmuxExec::new(
            Arc::clone(&runner),
            self.max_concurrent_tmux,
        ));
        let git: Arc<dyn GitOps> = Arc::new(SshGitOps::new(Arc::clone(&runner)));

        let mut guard = self.remote_ops.write().await;
        if let Some(ops) = guard.get(&key) {
            return Ok((Arc::clone(&ops.tmux), Arc::clone(&ops.git)));
        }
        guard.insert(
            key,
            RemoteOps {
                tmux: Arc::clone(&tmux),
                git: Arc::clone(&git),
            },
        );
        Ok((tmux, git))
    }

    /// Build a `StatusBarInfo` from session metadata
    pub fn status_bar_info(&self, session: &WorktreeSession, state: &AppState) -> StatusBarInfo {
        let project_name = state
            .get_project(&session.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        StatusBarInfo {
            branch: session.branch.clone(),
            pr_number: session.pr_number,
            pr_merged: session.pr_merged,
            status_style: self.tmux_status_style.clone(),
            is_shell: false,
            project_name,
        }
    }

    /// Generate branch name from title
    fn generate_branch_name(&self, title: &str) -> String {
        let sanitized = self.sanitize_name(title);

        let config = self.config_store.read();
        if config.branch_prefix.is_empty() {
            sanitized
        } else {
            format!("{}/{}", config.branch_prefix, sanitized)
        }
    }

    /// Sanitize a name for use as branch/directory name
    fn sanitize_name(&self, name: &str) -> String {
        sanitize_name(name)
    }
}

/// Sanitize a name for use as a branch/directory name.
///
/// Lowercases, replaces non-alphanumeric characters (except `-` and `_`) with
/// `-`, and trims leading/trailing `-`.
pub fn sanitize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Decide whether the `[branch]` annotation should be shown next to a session
/// title.
///
/// Returns `Some(branch)` when the branch carries information beyond what
/// the title already conveys. Returns `None` when:
/// - the title is literally identical to the branch (the checkout-branch
///   flow sets these equal — no point rendering the same string twice), or
/// - the branch matches `sanitize_name(title)` either exactly or as the
///   last `/`-segment (so a configured `branch_prefix` like `"user/"` is
///   treated as noise).
pub fn display_branch<'a>(title: &str, branch: &'a str) -> Option<&'a str> {
    if title == branch {
        return None;
    }
    let sanitized = sanitize_name(title);
    if sanitized.is_empty() {
        return Some(branch);
    }
    let matches = branch == sanitized
        || branch
            .rsplit_once('/')
            .map(|(_, tail)| tail == sanitized)
            .unwrap_or(false);
    if matches { None } else { Some(branch) }
}
