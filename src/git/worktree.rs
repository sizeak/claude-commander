//! Git worktree management
//!
//! Provides worktree lifecycle operations:
//! - Create worktree with new or existing branch
//! - Remove worktree
//! - List worktrees

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tracing::{debug, info, instrument, warn};

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
        let worktrees_dir = self.worktrees_dir.clone();
        let repo_path = self.backend.path().to_owned();

        // Check if branch exists (sync gix operation — done before any .await
        // so that non-Sync gix types don't cross await boundaries)
        let branch_exists = self.backend.branch_exists(branch_name)?;

        // All remaining work is async CLI commands that don't need &self
        Self::run_create_worktree(
            worktrees_dir,
            repo_path,
            worktree_path,
            branch_name.to_string(),
            branch_exists,
        )
        .await
    }

    /// Run the async portion of worktree creation (CLI commands only, no gix types).
    ///
    /// This is a standalone async function so that non-Sync gix types from
    /// the sync preparation phase are not held across await points, keeping
    /// the resulting future `Send`.
    pub async fn run_create_worktree(
        worktrees_dir: PathBuf,
        repo_path: PathBuf,
        worktree_path: PathBuf,
        branch_name: String,
        branch_exists: bool,
    ) -> Result<WorktreeInfo> {
        // Ensure worktrees directory exists
        tokio::fs::create_dir_all(&worktrees_dir)
            .await
            .map_err(|e| {
                GitError::WorktreeError(format!("Failed to create worktrees dir: {}", e))
            })?;

        let mut cmd = Command::new("git");
        cmd.current_dir(&repo_path).arg("worktree").arg("add");

        if branch_exists {
            // Checkout existing branch
            debug!("Branch {} exists, checking out", branch_name);
            cmd.arg(&worktree_path).arg(&branch_name);
        } else {
            // Create new branch
            debug!("Creating new branch {}", branch_name);
            cmd.arg("-b").arg(&branch_name).arg(&worktree_path);
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
            worktree_path, branch_name
        );

        // Copy .worktreeinclude files (best-effort)
        if let Err(e) = copy_worktree_includes(&repo_path, &worktree_path).await {
            warn!("Failed to copy worktree includes: {}", e);
        }

        // Get the HEAD of the new worktree
        let head = Self::get_worktree_head_static(&worktree_path).await?;

        Ok(WorktreeInfo {
            path: worktree_path,
            branch: branch_name,
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
            return Err(
                GitError::WorktreeError(format!("git worktree remove failed: {}", stderr)).into(),
            );
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
            return Err(
                GitError::WorktreeError(format!("git worktree list failed: {}", stderr)).into(),
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let worktrees = parse_worktree_list(&stdout)?;

        Ok(worktrees)
    }

    /// Get HEAD commit of a worktree
    async fn get_worktree_head_static(worktree_path: &Path) -> Result<String> {
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
            return Err(
                GitError::WorktreeError(format!("git worktree prune failed: {}", stderr)).into(),
            );
        }

        Ok(())
    }
}

/// Copy files matching `.worktreeinclude` patterns into a new worktree.
///
/// Runs two `git ls-files` commands against the repo root:
/// 1. All gitignored untracked files
/// 2. All files matching `.worktreeinclude` patterns
///
/// The intersection (files that are both gitignored AND match `.worktreeinclude`)
/// is copied into the worktree. Symlinks are skipped for security.
async fn copy_worktree_includes(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    let include_file = repo_path.join(".worktreeinclude");
    if !include_file.exists() {
        return Ok(());
    }

    // Run both git ls-files commands concurrently
    let (gitignored_result, included_result) = tokio::join!(
        Command::new("git")
            .current_dir(repo_path)
            .args([
                "ls-files",
                "--ignored",
                "--exclude-standard",
                "-o",
                "-z",
                "--directory",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
        Command::new("git")
            .current_dir(repo_path)
            .args([
                "ls-files",
                "--ignored",
                "--exclude-from=.worktreeinclude",
                "-o",
                "-z",
                "--directory",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    );

    let gitignored_output = gitignored_result
        .map_err(|e| GitError::WorktreeError(format!("Failed to list gitignored files: {e}")))?;
    let included_output = included_result.map_err(|e| {
        GitError::WorktreeError(format!("Failed to list worktreeinclude files: {e}"))
    })?;

    if !gitignored_output.status.success() {
        let stderr = String::from_utf8_lossy(&gitignored_output.stderr);
        warn!("git ls-files --exclude-standard failed: {}", stderr);
        return Ok(());
    }
    if !included_output.status.success() {
        let stderr = String::from_utf8_lossy(&included_output.stderr);
        warn!(
            "git ls-files --exclude-from=.worktreeinclude failed: {}",
            stderr
        );
        return Ok(());
    }

    let gitignored: HashSet<&str> = parse_nul_separated(&gitignored_output.stdout);
    let included: HashSet<&str> = parse_nul_separated(&included_output.stdout);

    let intersection: Vec<&str> = gitignored.intersection(&included).copied().collect();

    if intersection.is_empty() {
        return Ok(());
    }

    let mut copied = 0usize;
    for rel_path in &intersection {
        // Reject paths with .. components to prevent directory traversal
        if Path::new(rel_path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            warn!("Skipping path with '..' component: {}", rel_path);
            continue;
        }

        let source = repo_path.join(rel_path);
        let dest = worktree_path.join(rel_path);

        // Skip symlinks
        match tokio::fs::symlink_metadata(&source).await {
            Ok(meta) if meta.is_symlink() => {
                debug!("Skipping symlink: {}", rel_path);
                continue;
            }
            Err(e) => {
                warn!("Failed to stat {}: {}", rel_path, e);
                continue;
            }
            Ok(meta) => meta,
        };

        if source.is_dir() {
            match copy_dir_recursive(&source, &dest).await {
                Ok(n) => copied += n,
                Err(e) => warn!("Failed to copy directory {}: {}", rel_path, e),
            }
        } else {
            if let Some(parent) = dest.parent()
                && let Err(e) = tokio::fs::create_dir_all(parent).await
            {
                warn!("Failed to create parent dir for {}: {}", rel_path, e);
                continue;
            }
            match tokio::fs::copy(&source, &dest).await {
                Ok(_) => copied += 1,
                Err(e) => warn!("Failed to copy {}: {}", rel_path, e),
            }
        }
    }

    info!("Copied {} worktree-included files", copied);
    Ok(())
}

/// Parse NUL-separated output from `git ls-files -z`, stripping trailing slashes
/// from directory entries produced by `--directory`.
fn parse_nul_separated(bytes: &[u8]) -> HashSet<&str> {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return HashSet::new(),
    };
    text.split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.strip_suffix('/').unwrap_or(s))
        .collect()
}

/// Recursively copy a directory, skipping symlinks at every level.
/// Returns the number of files copied.
async fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<usize> {
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| GitError::WorktreeError(format!("Failed to create dir {dest:?}: {e}")))?;

    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| GitError::WorktreeError(format!("Failed to read dir {src:?}: {e}")))?;

    let mut count = 0;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| GitError::WorktreeError(format!("Failed to read dir entry: {e}")))?
    {
        let meta = match tokio::fs::symlink_metadata(entry.path()).await {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to stat {:?}: {}", entry.path(), e);
                continue;
            }
        };

        if meta.is_symlink() {
            continue;
        }

        let dest_entry = dest.join(entry.file_name());
        if meta.is_dir() {
            count += Box::pin(copy_dir_recursive(&entry.path(), &dest_entry)).await?;
        } else {
            if let Err(e) = tokio::fs::copy(entry.path(), &dest_entry).await {
                warn!("Failed to copy {:?}: {}", entry.path(), e);
                continue;
            }
            count += 1;
        }
    }

    Ok(count)
}

/// Parse git worktree list --porcelain output
fn parse_worktree_list(output: &str) -> Result<Vec<WorktreeInfo>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list() {
        let output = r#"worktree /path/to/main
HEAD abc123def456
branch refs/heads/main

worktree /path/to/feature
HEAD def456abc123
branch refs/heads/feature-branch
"#;

        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 2);

        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/main"));
        assert_eq!(worktrees[0].branch, "main");
        assert!(worktrees[0].is_main);

        assert_eq!(worktrees[1].path, PathBuf::from("/path/to/feature"));
        assert_eq!(worktrees[1].branch, "feature-branch");
        assert!(!worktrees[1].is_main);
    }

    #[test]
    fn test_parse_worktree_list_single_main() {
        let output = "worktree /path/to/main\nHEAD abc123\nbranch refs/heads/main\n";
        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].branch, "main");
    }

    #[test]
    fn test_parse_worktree_list_detached_head() {
        let output = "worktree /path/to/main\nHEAD abc123\nbranch refs/heads/main\n\nworktree /path/to/detached\nHEAD def456\n";
        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[1].branch, "HEAD");
    }

    #[test]
    fn test_parse_worktree_list_empty() {
        let worktrees = parse_worktree_list("").unwrap();
        assert!(worktrees.is_empty());
    }

    #[test]
    fn test_parse_nul_separated() {
        let input = b"foo\0bar/baz\0node_modules/\0";
        let result = parse_nul_separated(input);
        assert_eq!(
            result,
            HashSet::from(["foo", "bar/baz", "node_modules"])
        );
    }

    #[test]
    fn test_parse_nul_separated_empty() {
        let result = parse_nul_separated(b"");
        assert!(result.is_empty());
    }

    /// Helper to run a git command in a directory
    async fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Initialize a git repo with an initial commit
    async fn init_repo(dir: &Path) {
        git(dir, &["init"]).await;
        git(dir, &["config", "user.email", "test@test.com"]).await;
        git(dir, &["config", "user.name", "Test"]).await;
        // Create an initial commit so HEAD exists
        let placeholder = dir.join(".gitkeep");
        tokio::fs::write(&placeholder, "").await.unwrap();
        git(dir, &["add", ".gitkeep"]).await;
        git(dir, &["-c", "commit.gpgsign=false", "commit", "-m", "init"]).await;
    }

    #[tokio::test]
    async fn test_copy_worktree_includes_no_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        tokio::fs::create_dir(&repo).await.unwrap();
        init_repo(&repo).await;

        let worktree = tmp.path().join("wt");
        tokio::fs::create_dir(&worktree).await.unwrap();

        // No .worktreeinclude — should be a no-op
        let result = copy_worktree_includes(&repo, &worktree).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_copy_worktree_includes_empty_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        tokio::fs::create_dir(&repo).await.unwrap();
        init_repo(&repo).await;

        // Create empty .worktreeinclude
        tokio::fs::write(repo.join(".worktreeinclude"), "").await.unwrap();

        // Create a gitignored file
        tokio::fs::write(repo.join(".gitignore"), "*.log\n").await.unwrap();
        tokio::fs::write(repo.join("app.log"), "log content").await.unwrap();

        let worktree = tmp.path().join("wt");
        tokio::fs::create_dir(&worktree).await.unwrap();

        let result = copy_worktree_includes(&repo, &worktree).await;
        assert!(result.is_ok());
        // Empty .worktreeinclude matches nothing — file should not be copied
        assert!(!worktree.join("app.log").exists());
    }

    #[tokio::test]
    async fn test_copy_worktree_includes_intersection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        tokio::fs::create_dir(&repo).await.unwrap();
        init_repo(&repo).await;

        // .gitignore ignores *.log and .env
        tokio::fs::write(repo.join(".gitignore"), "*.log\n.env\n").await.unwrap();
        // .worktreeinclude only wants *.log
        tokio::fs::write(repo.join(".worktreeinclude"), "*.log\n").await.unwrap();

        // Create files
        tokio::fs::write(repo.join("app.log"), "log data").await.unwrap();
        tokio::fs::write(repo.join(".env"), "SECRET=x").await.unwrap();
        tokio::fs::create_dir(repo.join("src")).await.unwrap();
        tokio::fs::write(repo.join("src/main.rs"), "fn main(){}").await.unwrap();

        // Commit tracked files so git knows about them
        git(&repo, &["add", ".gitignore", ".worktreeinclude", "src/main.rs"]).await;
        git(&repo, &["-c", "commit.gpgsign=false", "commit", "-m", "add files"]).await;

        let worktree = tmp.path().join("wt");
        tokio::fs::create_dir(&worktree).await.unwrap();

        let result = copy_worktree_includes(&repo, &worktree).await;
        assert!(result.is_ok());

        // app.log is gitignored AND in .worktreeinclude → copied
        assert!(worktree.join("app.log").exists());
        let content = tokio::fs::read_to_string(worktree.join("app.log")).await.unwrap();
        assert_eq!(content, "log data");

        // .env is gitignored but NOT in .worktreeinclude → not copied
        assert!(!worktree.join(".env").exists());

        // src/main.rs is tracked, not gitignored → not copied
        assert!(!worktree.join("src/main.rs").exists());
    }

    #[tokio::test]
    async fn test_copy_worktree_includes_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        tokio::fs::create_dir(&repo).await.unwrap();
        init_repo(&repo).await;

        // .gitignore ignores node_modules/
        tokio::fs::write(repo.join(".gitignore"), "node_modules/\n").await.unwrap();
        // .worktreeinclude includes node_modules/
        tokio::fs::write(repo.join(".worktreeinclude"), "node_modules/\n").await.unwrap();

        // Create node_modules with nested structure
        let nm = repo.join("node_modules");
        tokio::fs::create_dir_all(nm.join("pkg/lib")).await.unwrap();
        tokio::fs::write(nm.join("pkg/package.json"), r#"{"name":"pkg"}"#).await.unwrap();
        tokio::fs::write(nm.join("pkg/lib/index.js"), "module.exports = {}").await.unwrap();

        git(&repo, &["add", ".gitignore", ".worktreeinclude"]).await;
        git(&repo, &["-c", "commit.gpgsign=false", "commit", "-m", "add files"]).await;

        let worktree = tmp.path().join("wt");
        tokio::fs::create_dir(&worktree).await.unwrap();

        let result = copy_worktree_includes(&repo, &worktree).await;
        assert!(result.is_ok());

        // Entire directory tree should be copied
        assert!(worktree.join("node_modules/pkg/package.json").exists());
        assert!(worktree.join("node_modules/pkg/lib/index.js").exists());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_copy_worktree_includes_symlink_skipped() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        tokio::fs::create_dir(&repo).await.unwrap();
        init_repo(&repo).await;

        tokio::fs::write(repo.join(".gitignore"), "build/\n").await.unwrap();
        tokio::fs::write(repo.join(".worktreeinclude"), "build/\n").await.unwrap();

        // Create build dir with a real file and a symlink
        let build = repo.join("build");
        tokio::fs::create_dir(&build).await.unwrap();
        tokio::fs::write(build.join("output.bin"), "binary").await.unwrap();
        symlink("/etc/passwd", build.join("sneaky_link")).unwrap();

        git(&repo, &["add", ".gitignore", ".worktreeinclude"]).await;
        git(&repo, &["-c", "commit.gpgsign=false", "commit", "-m", "add files"]).await;

        let worktree = tmp.path().join("wt");
        tokio::fs::create_dir(&worktree).await.unwrap();

        let result = copy_worktree_includes(&repo, &worktree).await;
        assert!(result.is_ok());

        // Real file is copied
        assert!(worktree.join("build/output.bin").exists());
        // Symlink is NOT copied
        assert!(!worktree.join("build/sneaky_link").exists());
    }
}
