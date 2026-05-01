//! `GitOps` trait — abstraction over how git commands are dispatched.
//!
//! Concrete impls live in sibling modules:
//! - [`LocalGitOps`] — uses `gix` for sync reads and `git` CLI for mutations,
//!   all on the local filesystem.
//! - `SshGitOps` (added later) — runs `git -C <path> ...` over a persistent
//!   SSH session; no gix on remote paths.
//!
//! `SessionManager` holds one `Arc<dyn GitOps>` per project (resolved by
//! [`Project::location`](crate::session::Project::location) once that lands)
//! and routes every git operation through it.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, info, instrument, warn};

use super::worktree_include::copy_worktree_includes;
use super::{DiffInfo, GitBackend, WorktreeInfo, compute_diff_for_path, parse_worktree_list};
use crate::error::{GitError, Result};

/// Repository metadata returned by [`GitOps::discover`].
#[derive(Debug, Clone)]
pub struct DiscoveredRepo {
    /// Canonical path to the repo (may equal the input path for remotes).
    pub canonical_path: PathBuf,
    /// Repo display name (basename of the repo path).
    pub name: String,
    /// Detected main branch (defaults to remote HEAD, then `main`/`master`).
    pub main_branch: String,
}

/// Trait abstracting git command dispatch.
///
/// Methods are organized into three groups:
/// - **Discovery / metadata**: `discover`, `detect_main_branch`, `head_commit`.
/// - **Refs**: `branch_exists`, `ref_exists`, `list_branches`.
/// - **Mutations**: `fetch_origin`, `worktree_add`, `worktree_remove`,
///   `prune_worktrees`, `list_worktrees`, `compute_diff`.
#[async_trait]
pub trait GitOps: Send + Sync {
    /// Discover the git repo containing `path` and return its canonical
    /// metadata. Used when adding a project.
    async fn discover(&self, path: &Path) -> Result<DiscoveredRepo>;

    /// `git fetch origin`. Errors are logged but not propagated, matching
    /// the existing inline behavior in `finalize_session` — a stale local
    /// state shouldn't block session creation.
    async fn fetch_origin(&self, repo: &Path) -> Result<()>;

    async fn branch_exists(&self, repo: &Path, branch: &str) -> Result<bool>;
    async fn ref_exists(&self, repo: &Path, ref_name: &str) -> Result<bool>;
    async fn list_branches(&self, repo: &Path) -> Result<Vec<(String, bool)>>;
    async fn detect_main_branch(&self, repo: &Path) -> Result<String>;
    async fn head_commit(&self, repo: &Path) -> Result<String>;

    /// Create a new worktree at `worktree_path`. If `branch_exists` is true,
    /// checks out that branch; otherwise creates it (optionally from
    /// `start_point`). Returns the populated `WorktreeInfo`.
    async fn worktree_add(
        &self,
        repo: &Path,
        worktree_path: &Path,
        branch: &str,
        branch_exists: bool,
        start_point: Option<&str>,
    ) -> Result<WorktreeInfo>;

    async fn worktree_remove(&self, repo: &Path, worktree_path: &Path, force: bool) -> Result<()>;
    async fn list_worktrees(&self, repo: &Path) -> Result<Vec<WorktreeInfo>>;
    async fn prune_worktrees(&self, repo: &Path) -> Result<()>;

    /// Compute a diff for the given worktree (cached upstream by
    /// `DiffCache`). Includes untracked files via `git ls-files`.
    async fn compute_diff(&self, worktree_path: &Path) -> Result<DiffInfo>;
}

/// Local-process git ops: `gix` for sync reads, `git` CLI for mutations.
#[derive(Default, Clone)]
pub struct LocalGitOps;

impl LocalGitOps {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl GitOps for LocalGitOps {
    #[instrument(skip(self))]
    async fn discover(&self, path: &Path) -> Result<DiscoveredRepo> {
        let backend = GitBackend::discover(path)?;
        let main_branch = backend.detect_main_branch()?;
        let name = backend.repo_name();
        let canonical_path =
            std::fs::canonicalize(backend.path()).unwrap_or_else(|_| backend.path().to_path_buf());
        Ok(DiscoveredRepo {
            canonical_path,
            name,
            main_branch,
        })
    }

    #[instrument(skip(self))]
    async fn fetch_origin(&self, repo: &Path) -> Result<()> {
        let output = Command::new("git")
            .current_dir(repo)
            .args(["fetch", "origin"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("git fetch failed (continuing anyway): {}", stderr);
        }
        Ok(())
    }

    async fn branch_exists(&self, repo: &Path, branch: &str) -> Result<bool> {
        // gix Repository is !Send — keep it scoped within this fn so we
        // never hold it across an .await point.
        let backend = GitBackend::open(repo)?;
        backend.branch_exists(branch)
    }

    async fn ref_exists(&self, repo: &Path, ref_name: &str) -> Result<bool> {
        let backend = GitBackend::open(repo)?;
        backend.ref_exists(ref_name)
    }

    async fn list_branches(&self, repo: &Path) -> Result<Vec<(String, bool)>> {
        let backend = GitBackend::open(repo)?;
        backend.list_branches()
    }

    async fn detect_main_branch(&self, repo: &Path) -> Result<String> {
        let backend = GitBackend::open(repo)?;
        backend.detect_main_branch()
    }

    async fn head_commit(&self, repo: &Path) -> Result<String> {
        let backend = GitBackend::open(repo)?;
        backend.head_commit_id()
    }

    #[instrument(skip(self))]
    async fn worktree_add(
        &self,
        repo: &Path,
        worktree_path: &Path,
        branch: &str,
        branch_exists: bool,
        start_point: Option<&str>,
    ) -> Result<WorktreeInfo> {
        // Ensure parent directory exists so `git worktree add` doesn't fail
        // when a freshly configured worktrees_dir doesn't yet exist.
        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                GitError::WorktreeError(format!("Failed to create worktrees dir: {}", e))
            })?;
        }

        let mut cmd = Command::new("git");
        cmd.current_dir(repo).arg("worktree").arg("add");

        if branch_exists {
            debug!("Branch {} exists, checking out", branch);
            if start_point.is_some() {
                debug!("Ignoring start_point for existing branch {}", branch);
            }
            cmd.arg(worktree_path).arg(branch);
        } else {
            debug!("Creating new branch {}", branch);
            cmd.arg("-b").arg(branch).arg(worktree_path);
            if let Some(sp) = start_point {
                debug!("Using start point {}", sp);
                cmd.arg(sp);
            }
        }

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| GitError::WorktreeError(format!("Failed to run git worktree: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(
                GitError::WorktreeError(format!("git worktree add failed: {}", stderr)).into(),
            );
        }

        info!(
            "Created worktree at {:?} with branch {}",
            worktree_path, branch
        );

        if let Err(e) = copy_worktree_includes(repo, worktree_path).await {
            warn!("Failed to copy worktree includes: {}", e);
        }

        let head = head_of_worktree(worktree_path).await.unwrap_or_default();

        Ok(WorktreeInfo {
            path: worktree_path.to_path_buf(),
            branch: branch.to_string(),
            head,
            is_main: false,
        })
    }

    #[instrument(skip(self))]
    async fn worktree_remove(&self, repo: &Path, worktree_path: &Path, force: bool) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.current_dir(repo).arg("worktree").arg("remove");
        if force {
            cmd.arg("--force");
        }
        cmd.arg(worktree_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| GitError::WorktreeError(format!("Failed to run git worktree: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(
                GitError::WorktreeError(format!("git worktree remove failed: {}", stderr)).into(),
            );
        }
        info!("Removed worktree at {:?}", worktree_path);
        Ok(())
    }

    async fn list_worktrees(&self, repo: &Path) -> Result<Vec<WorktreeInfo>> {
        let output = Command::new("git")
            .current_dir(repo)
            .args(["worktree", "list", "--porcelain"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| GitError::WorktreeError(format!("Failed to list worktrees: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(
                GitError::WorktreeError(format!("git worktree list failed: {}", stderr)).into(),
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_worktree_list(&stdout)
    }

    async fn prune_worktrees(&self, repo: &Path) -> Result<()> {
        let output = Command::new("git")
            .current_dir(repo)
            .args(["worktree", "prune"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| GitError::WorktreeError(format!("Failed to prune worktrees: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(
                GitError::WorktreeError(format!("git worktree prune failed: {}", stderr)).into(),
            );
        }
        Ok(())
    }

    async fn compute_diff(&self, worktree_path: &Path) -> Result<DiffInfo> {
        compute_diff_for_path(worktree_path).await
    }
}

/// Get HEAD commit of a worktree by shelling `git rev-parse HEAD` in it.
async fn head_of_worktree(worktree_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .current_dir(worktree_path)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| GitError::WorktreeError(format!("Failed to get HEAD: {}", e)))?;
    if !output.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
