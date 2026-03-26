//! Git backend using pure gitoxide
//!
//! Provides git operations without any CLI dependencies.

use std::path::{Path, PathBuf};

use gix::Repository;
use tracing::{debug, instrument};

use crate::error::{GitError, Result};

/// Git backend using gitoxide
///
/// Provides all git operations through pure Rust implementation.
pub struct GitBackend {
    /// The gitoxide repository handle
    repo: Repository,
    /// Path to the repository
    path: PathBuf,
}

impl GitBackend {
    /// Open an existing repository
    #[instrument(skip_all, fields(path = %path.as_ref().display()))]
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        let repo = gix::open(&path).map_err(|e| {
            if e.to_string().contains("not a git repository") {
                GitError::NotARepository(path.clone())
            } else {
                GitError::Gix(e.to_string())
            }
        })?;

        debug!("Opened repository at {:?}", path);

        Ok(Self { repo, path })
    }

    /// Discover repository from a path (searches parent directories)
    #[instrument(skip_all, fields(path = %path.as_ref().display()))]
    pub fn discover(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        let repo = gix::discover(path).map_err(|_e| {
            GitError::NotARepository(path.to_path_buf())
        })?;

        let repo_path = repo.path().parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf());

        debug!("Discovered repository at {:?}", repo_path);

        Ok(Self {
            repo,
            path: repo_path,
        })
    }

    /// Get the repository path
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the current branch name
    pub fn current_branch(&self) -> Result<String> {
        let head = self.repo.head().map_err(|e| GitError::Gix(e.to_string()))?;

        match head.kind {
            gix::head::Kind::Symbolic(reference) => {
                // Get shortened name from the reference
                let name = reference.name.shorten().to_string();
                Ok(name)
            }
            gix::head::Kind::Detached { .. } => {
                // Return short commit ID for detached HEAD
                match head.id() {
                    Some(id) => {
                        let id_str = id.to_string();
                        let short = if id_str.len() > 8 { &id_str[..8] } else { &id_str };
                        Ok(format!("HEAD detached at {}", short))
                    }
                    None => Ok("HEAD (no commits)".to_string()),
                }
            }
            gix::head::Kind::Unborn(full_name) => {
                // Unborn branch - the full_name IS the reference name (a FullName)
                let name = full_name.shorten().to_string();
                Ok(name)
            }
        }
    }

    /// Check if a branch exists
    pub fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        let refs = self.repo.references().map_err(|e| GitError::Gix(e.to_string()))?;

        let branch_ref = format!("refs/heads/{}", branch_name);

        for reference in refs.all().map_err(|e| GitError::Gix(e.to_string()))? {
            match reference {
                Ok(r) => {
                    if r.name().as_bstr() == branch_ref.as_bytes() {
                        return Ok(true);
                    }
                }
                Err(_) => continue,
            }
        }

        Ok(false)
    }

    /// Get the HEAD commit ID
    pub fn head_commit_id(&self) -> Result<String> {
        let head = self.repo.head().map_err(|e| GitError::Gix(e.to_string()))?;
        match head.id() {
            Some(id) => Ok(id.to_string()),
            None => Err(GitError::InvalidRef("HEAD has no commits".to_string()).into()),
        }
    }

    /// Check if the working directory is dirty
    pub fn is_dirty(&self) -> Result<bool> {
        // Get the index
        let _index = self.repo.index().map_err(|e| GitError::Gix(e.to_string()))?;

        // For now, we'll use a simple heuristic: check if there are any changes
        // A full implementation would compare index to HEAD and worktree to index

        // This is a simplified check - in practice you'd want to use gix-status
        // which provides full status information
        Ok(false) // Placeholder - full implementation needed
    }

    /// Get the main branch name (main or master)
    pub fn detect_main_branch(&self) -> Result<String> {
        // Check for 'main' first, then 'master'
        if self.branch_exists("main")? {
            Ok("main".to_string())
        } else if self.branch_exists("master")? {
            Ok("master".to_string())
        } else {
            // Fall back to current branch
            self.current_branch()
        }
    }

    /// Get the repository name (directory name)
    pub fn repo_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    }

    /// Get the gitoxide repository handle
    pub fn repo(&self) -> &Repository {
        &self.repo
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_test_repo() -> (TempDir, GitBackend) {
        let temp_dir = TempDir::new().unwrap();

        // Initialize a git repository using gix
        let repo = gix::init(temp_dir.path()).unwrap();

        let backend = GitBackend {
            repo,
            path: temp_dir.path().to_path_buf(),
        };

        (temp_dir, backend)
    }

    #[test]
    fn test_repo_name() {
        let (_temp, backend) = init_test_repo();
        // TempDir creates random names, so just check it's not empty
        assert!(!backend.repo_name().is_empty());
    }

    #[test]
    fn test_detect_main_branch_unborn() {
        let (_temp, backend) = init_test_repo();
        // Newly initialized repo has unborn 'main' or 'master' branch
        let branch = backend.detect_main_branch();
        // This should not error, even for unborn branches
        assert!(branch.is_ok());
    }
}
