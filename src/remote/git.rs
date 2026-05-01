//! SSH-backed [`GitOps`](crate::git::GitOps) implementation.
//!
//! Dispatches `git -C <path> ...` through a [`RemoteRunner`]. Works against
//! plain SSH hosts (via `OpensshRunner`) and Codespaces (via
//! `GhCodespaceRunner`) without changes.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, instrument, warn};

use super::runner::RemoteRunner;
use crate::error::{GitError, Result};
use crate::git::{
    DiffInfo, DiscoveredRepo, GitOps, WorktreeInfo, parse_diff_stat, parse_worktree_list,
};

/// SSH-backed git operations.
pub struct SshGitOps {
    runner: Arc<dyn RemoteRunner>,
}

impl SshGitOps {
    pub fn new(runner: Arc<dyn RemoteRunner>) -> Self {
        Self { runner }
    }

    /// Run `git -C <repo> <args...>` over the runner. Returns stdout on success.
    /// On failure, surfaces both stdout and stderr in the error message —
    /// some git/hook errors print to stdout, some to stderr, and gh sometimes
    /// merges them; better to show everything than guess wrong.
    async fn git(&self, repo: &Path, args: &[&str]) -> Result<String> {
        let repo_str = repo
            .to_str()
            .ok_or_else(|| GitError::WorktreeError("non-UTF-8 repo path".to_string()))?;
        let mut argv: Vec<&str> = vec!["git", "-C", repo_str];
        argv.extend_from_slice(args);
        let output = self.runner.run(&argv).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut detail = String::new();
            if !stderr.trim().is_empty() {
                detail.push_str(stderr.trim());
            }
            if !stdout.trim().is_empty() {
                if !detail.is_empty() {
                    detail.push_str(" / ");
                }
                detail.push_str(stdout.trim());
            }
            if detail.is_empty() {
                detail.push_str(&format!("(no output, exit {:?})", output.status.code()));
            }
            return Err(GitError::WorktreeError(format!(
                "git {} failed: {}",
                args.join(" "),
                detail
            ))
            .into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Run a git command and return whether it exited zero (without
    /// converting non-zero into an `Err`). Used for ref-existence checks
    /// where exit status is the answer.
    async fn git_exists(&self, repo: &Path, args: &[&str]) -> Result<bool> {
        let repo_str = repo
            .to_str()
            .ok_or_else(|| GitError::WorktreeError("non-UTF-8 repo path".to_string()))?;
        let mut argv: Vec<&str> = vec!["git", "-C", repo_str];
        argv.extend_from_slice(args);
        let output = self.runner.run(&argv).await?;
        Ok(output.status.success())
    }
}

#[async_trait]
impl GitOps for SshGitOps {
    async fn discover(&self, path: &Path) -> Result<DiscoveredRepo> {
        let root = self
            .git(path, &["rev-parse", "--show-toplevel"])
            .await?
            .trim()
            .to_string();
        let canonical_path = std::path::PathBuf::from(&root);
        let name = canonical_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let main_branch = self.detect_main_branch(&canonical_path).await?;
        Ok(DiscoveredRepo {
            canonical_path,
            name,
            main_branch,
        })
    }

    #[instrument(skip(self))]
    async fn fetch_origin(&self, repo: &Path) -> Result<()> {
        if let Err(e) = self.git(repo, &["fetch", "origin"]).await {
            warn!("ssh git fetch failed (continuing anyway): {}", e);
        }
        Ok(())
    }

    async fn branch_exists(&self, repo: &Path, branch: &str) -> Result<bool> {
        let ref_name = format!("refs/heads/{}", branch);
        self.git_exists(repo, &["show-ref", "--quiet", "--verify", &ref_name])
            .await
    }

    async fn ref_exists(&self, repo: &Path, ref_name: &str) -> Result<bool> {
        self.git_exists(repo, &["show-ref", "--quiet", "--verify", ref_name])
            .await
    }

    async fn list_branches(&self, repo: &Path) -> Result<Vec<(String, bool)>> {
        let local = self
            .git(
                repo,
                &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
            )
            .await?;
        let remote = self
            .git(
                repo,
                &["for-each-ref", "--format=%(refname:short)", "refs/remotes/"],
            )
            .await?;

        let mut out: Vec<(String, bool)> = Vec::new();
        let mut local_names: Vec<&str> = local.lines().filter(|l| !l.is_empty()).collect();
        local_names.sort();
        local_names.dedup();
        for n in local_names {
            out.push((n.to_string(), false));
        }
        let mut remote_names: Vec<&str> = remote
            .lines()
            .filter(|l| !l.is_empty() && !l.ends_with("/HEAD"))
            .collect();
        remote_names.sort();
        remote_names.dedup();
        for n in remote_names {
            out.push((n.to_string(), true));
        }
        Ok(out)
    }

    async fn detect_main_branch(&self, repo: &Path) -> Result<String> {
        if let Ok(out) = self
            .git(
                repo,
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            )
            .await
        {
            let trimmed = out.trim();
            if let Some(stripped) = trimmed.strip_prefix("origin/") {
                return Ok(stripped.to_string());
            }
        }
        if self.branch_exists(repo, "main").await.unwrap_or(false) {
            return Ok("main".to_string());
        }
        if self.branch_exists(repo, "master").await.unwrap_or(false) {
            return Ok("master".to_string());
        }
        let cur = self
            .git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await?
            .trim()
            .to_string();
        Ok(cur)
    }

    async fn head_commit(&self, repo: &Path) -> Result<String> {
        let out = self.git(repo, &["rev-parse", "HEAD"]).await?;
        Ok(out.trim().to_string())
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
        // Ensure the worktree's parent dir exists on the remote.
        if let Some(parent) = worktree_path.parent()
            && let Some(parent_str) = parent.to_str()
        {
            let _ = self.runner.run(&["mkdir", "-p", parent_str]).await;
        }

        let worktree_str = worktree_path
            .to_str()
            .ok_or_else(|| GitError::WorktreeError("non-UTF-8 worktree path".to_string()))?;

        let mut args: Vec<&str> = vec!["worktree", "add"];
        if branch_exists {
            debug!("Remote branch {} exists, checking out", branch);
            args.push(worktree_str);
            args.push(branch);
        } else {
            debug!("Creating remote branch {}", branch);
            args.push("-b");
            args.push(branch);
            args.push(worktree_str);
            if let Some(sp) = start_point {
                debug!("Using start point {}", sp);
                args.push(sp);
            }
        }
        self.git(repo, &args).await?;

        let head = self
            .git(worktree_path, &["rev-parse", "HEAD"])
            .await
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        Ok(WorktreeInfo {
            path: worktree_path.to_path_buf(),
            branch: branch.to_string(),
            head,
            is_main: false,
        })
    }

    async fn worktree_remove(&self, repo: &Path, worktree_path: &Path, force: bool) -> Result<()> {
        let worktree_str = worktree_path
            .to_str()
            .ok_or_else(|| GitError::WorktreeError("non-UTF-8 worktree path".to_string()))?;
        let args: Vec<&str> = if force {
            vec!["worktree", "remove", "--force", worktree_str]
        } else {
            vec!["worktree", "remove", worktree_str]
        };
        self.git(repo, &args).await?;
        Ok(())
    }

    async fn list_worktrees(&self, repo: &Path) -> Result<Vec<WorktreeInfo>> {
        let out = self.git(repo, &["worktree", "list", "--porcelain"]).await?;
        parse_worktree_list(&out)
    }

    async fn prune_worktrees(&self, repo: &Path) -> Result<()> {
        self.git(repo, &["worktree", "prune"]).await?;
        Ok(())
    }

    async fn compute_diff(&self, worktree_path: &Path) -> Result<DiffInfo> {
        // Three sequential remote reads — total wall time is ~3 RTTs, fine
        // at the upstream 500ms cache TTL. Untracked-file diffs are skipped
        // for v1; tracked changes only.
        let diff = self
            .git(worktree_path, &["diff", "HEAD"])
            .await
            .unwrap_or_default();
        let stat = self
            .git(worktree_path, &["diff", "--stat", "HEAD"])
            .await
            .unwrap_or_default();
        let untracked = self
            .git(
                worktree_path,
                &["ls-files", "--others", "--exclude-standard"],
            )
            .await
            .unwrap_or_default();

        let (mut files_changed, lines_added, lines_removed) = parse_diff_stat(&stat);
        files_changed += untracked.lines().filter(|l| !l.is_empty()).count();
        let line_count = diff.lines().count();

        Ok(DiffInfo {
            diff,
            files_changed,
            lines_added,
            lines_removed,
            line_count,
            computed_at: std::time::Instant::now(),
            base_commit: "HEAD".to_string(),
        })
    }
}
