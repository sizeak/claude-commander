//! Git worktree management
//!
//! Provides worktree lifecycle operations:
//! - Create worktree with new or existing branch
//! - Remove worktree
//! - List worktrees

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tracing::{debug, info, instrument};

use super::GitBackend;
use crate::error::{GitError, Result};

/// Worktree information
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Path to the worktree
    pub path: PathBuf,
    /// Branch name
    pub branch: String,
    /// HEAD commit ID
    pub head: String,
    /// Whether this is the main worktree
    pub is_main: bool,
}

/// Worktree manager
///
/// Handles git worktree operations for session isolation.
///
/// Note: gitoxide's worktree support is still evolving, so we use
/// a hybrid approach: gitoxide for read operations, git CLI for mutations.
pub struct WorktreeManager {
    /// Git backend
    backend: GitBackend,
    /// Base directory for worktrees
    worktrees_dir: PathBuf,
}

impl WorktreeManager {
    /// Create a new worktree manager
    pub fn new(backend: GitBackend, worktrees_dir: PathBuf) -> Self {
        Self {
            backend,
            worktrees_dir,
        }
    }

    /// Get the repository path
    pub fn repo_path(&self) -> &Path {
        self.backend.path()
    }

    /// Get the worktrees directory
    pub fn worktrees_dir(&self) -> &Path {
        &self.worktrees_dir
    }

    /// Create a new worktree
    ///
    /// If the branch exists, checks it out into the worktree.
    /// If the branch doesn't exist, creates it from HEAD.
    #[instrument(skip(self))]
    pub async fn create_worktree(
        &self,
        worktree_name: &str,
        branch_name: &str,
    ) -> Result<WorktreeInfo> {
        let worktree_path = self.worktrees_dir.join(worktree_name);

        // Ensure worktrees directory exists
        tokio::fs::create_dir_all(&self.worktrees_dir)
            .await
            .map_err(|e| GitError::WorktreeError(format!("Failed to create worktrees dir: {}", e)))?;

        // Check if branch exists
        let branch_exists = self.backend.branch_exists(branch_name)?;

        let mut cmd = Command::new("git");
        cmd.current_dir(self.backend.path())
            .arg("worktree")
            .arg("add");

        if branch_exists {
            // Checkout existing branch
            debug!("Branch {} exists, checking out", branch_name);
            cmd.arg(&worktree_path).arg(branch_name);
        } else {
            // Create new branch
            debug!("Creating new branch {}", branch_name);
            cmd.arg("-b").arg(branch_name).arg(&worktree_path);
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
            return Err(GitError::WorktreeError(format!(
                "git worktree add failed: {}",
                stderr
            ))
            .into());
        }

        info!(
            "Created worktree at {:?} with branch {}",
            worktree_path, branch_name
        );

        // Get the HEAD of the new worktree
        let head = self.get_worktree_head(&worktree_path).await?;

        Ok(WorktreeInfo {
            path: worktree_path,
            branch: branch_name.to_string(),
            head,
            is_main: false,
        })
    }

    /// Remove a worktree
    #[instrument(skip(self))]
    pub async fn remove_worktree(&self, worktree_path: &Path, force: bool) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.current_dir(self.backend.path())
            .arg("worktree")
            .arg("remove");

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
            return Err(GitError::WorktreeError(format!(
                "git worktree remove failed: {}",
                stderr
            ))
            .into());
        }

        info!("Removed worktree at {:?}", worktree_path);
        Ok(())
    }

    /// List all worktrees
    #[instrument(skip(self))]
    pub async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let output = Command::new("git")
            .current_dir(self.backend.path())
            .args(["worktree", "list", "--porcelain"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| GitError::WorktreeError(format!("Failed to list worktrees: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::WorktreeError(format!(
                "git worktree list failed: {}",
                stderr
            ))
            .into());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let worktrees = self.parse_worktree_list(&stdout)?;

        Ok(worktrees)
    }

    /// Parse git worktree list --porcelain output
    fn parse_worktree_list(&self, output: &str) -> Result<Vec<WorktreeInfo>> {
        let mut worktrees = Vec::new();
        let mut current_path: Option<PathBuf> = None;
        let mut current_head: Option<String> = None;
        let mut current_branch: Option<String> = None;
        let mut is_main = true; // First worktree is main

        for line in output.lines() {
            if line.starts_with("worktree ") {
                // Save previous worktree if complete
                if let (Some(path), Some(head)) = (current_path.take(), current_head.take()) {
                    worktrees.push(WorktreeInfo {
                        path,
                        branch: current_branch.take().unwrap_or_else(|| "HEAD".to_string()),
                        head,
                        is_main,
                    });
                    is_main = false;
                }

                current_path = Some(PathBuf::from(line.trim_start_matches("worktree ")));
            } else if line.starts_with("HEAD ") {
                current_head = Some(line.trim_start_matches("HEAD ").to_string());
            } else if line.starts_with("branch ") {
                let branch = line.trim_start_matches("branch refs/heads/");
                current_branch = Some(branch.to_string());
            }
        }

        // Don't forget the last worktree
        if let (Some(path), Some(head)) = (current_path, current_head) {
            worktrees.push(WorktreeInfo {
                path,
                branch: current_branch.unwrap_or_else(|| "HEAD".to_string()),
                head,
                is_main,
            });
        }

        Ok(worktrees)
    }

    /// Get HEAD commit of a worktree
    async fn get_worktree_head(&self, worktree_path: &Path) -> Result<String> {
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
            return Ok("unknown".to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Prune stale worktree references
    pub async fn prune(&self) -> Result<()> {
        let output = Command::new("git")
            .current_dir(self.backend.path())
            .args(["worktree", "prune"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| GitError::WorktreeError(format!("Failed to prune worktrees: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::WorktreeError(format!(
                "git worktree prune failed: {}",
                stderr
            ))
            .into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list() {
        let backend = GitBackend::open(".").unwrap_or_else(|_| {
            // Create a minimal backend for testing parse logic
            panic!("Test requires a git repository");
        });

        let manager = WorktreeManager::new(backend, PathBuf::from("/tmp/worktrees"));

        let output = r#"worktree /path/to/main
HEAD abc123def456
branch refs/heads/main

worktree /path/to/feature
HEAD def456abc123
branch refs/heads/feature-branch
"#;

        let worktrees = manager.parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 2);

        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/main"));
        assert_eq!(worktrees[0].branch, "main");
        assert!(worktrees[0].is_main);

        assert_eq!(worktrees[1].path, PathBuf::from("/path/to/feature"));
        assert_eq!(worktrees[1].branch, "feature-branch");
        assert!(!worktrees[1].is_main);
    }
}
