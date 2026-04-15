//! Project lifecycle: add and remove git repositories.

use super::*;

impl SessionManager {
    /// Add a new project (git repository)
    #[instrument(skip(self))]
    pub async fn add_project(&self, repo_path: PathBuf) -> Result<ProjectId> {
        // Discover git repository
        let backend = GitBackend::discover(&repo_path)?;
        let main_branch = backend.detect_main_branch()?;
        let name = backend.repo_name();

        info!("Adding project '{}' from {:?}", name, repo_path);

        let repo_path =
            std::fs::canonicalize(backend.path()).unwrap_or_else(|_| backend.path().to_path_buf());
        let project = Project::new(name, repo_path, main_branch);
        let project_id = project.id;

        self.store
            .mutate(move |state| {
                state.add_project(project);
            })
            .await?;

        // Import any existing worktrees as sessions
        if let Err(e) = self.sync_worktrees(&project_id).await {
            warn!("Failed to sync worktrees on project add: {}", e);
        }

        Ok(project_id)
    }

    /// Scan a directory for git repositories and add them as projects.
    ///
    /// Walks the directory tree recursively. When a `.git` directory is found
    /// the repo is registered and that subtree is not descended further.
    /// Repos that already exist (matched by canonicalized git root) are skipped.
    #[instrument(skip(self))]
    pub async fn scan_directory(&self, dir: &Path) -> Result<ScanResult> {
        // Collect all existing repo paths for duplicate detection
        let existing_paths: std::collections::HashSet<PathBuf> = {
            let state = self.store.read().await;
            state
                .projects
                .values()
                .map(|p| p.repo_path.clone())
                .collect()
        };

        // Walk the directory tree and collect git repo roots
        let repo_paths = Self::find_git_repos(dir);
        info!("Found {} git repositories in {:?}", repo_paths.len(), dir);

        let mut added = 0;
        let mut skipped = 0;

        for repo_path in repo_paths {
            // Resolve to canonical git root for duplicate detection
            let canonical = match GitBackend::discover(&repo_path) {
                Ok(backend) => std::fs::canonicalize(backend.path())
                    .unwrap_or_else(|_| backend.path().to_path_buf()),
                Err(e) => {
                    debug!("Skipping {:?}: {}", repo_path, e);
                    continue;
                }
            };

            if existing_paths.contains(&canonical) {
                skipped += 1;
                continue;
            }

            match self.add_project(repo_path).await {
                Ok(_) => added += 1,
                Err(e) => {
                    debug!("Failed to add {:?}: {}", canonical, e);
                }
            }
        }

        Ok(ScanResult { added, skipped })
    }

    /// Walk a directory tree and collect paths that contain a `.git` directory.
    /// Prunes subtrees once a `.git` is found (does not descend into repos).
    fn find_git_repos(dir: &Path) -> Vec<PathBuf> {
        let mut repos = Vec::new();
        let mut stack = vec![dir.to_path_buf()];

        while let Some(current) = stack.pop() {
            let entries = match std::fs::read_dir(&current) {
                Ok(entries) => entries,
                Err(_) => continue, // permission denied or other error — skip silently
            };

            let mut is_git_repo = false;
            let mut subdirs = Vec::new();

            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if entry.file_name() == ".git" {
                        is_git_repo = true;
                        // Don't break — we still want to avoid descending
                    } else {
                        subdirs.push(path);
                    }
                }
            }

            if is_git_repo {
                repos.push(current);
                // Prune: don't descend into this repo's subdirectories
            } else {
                // Not a repo — continue descending
                stack.extend(subdirs);
            }
        }

        repos
    }

    /// Remove a project and all its sessions
    #[instrument(skip(self))]
    pub async fn remove_project(&self, project_id: &ProjectId) -> Result<()> {
        // Read project data for tmux cleanup
        let project = {
            let state = self.store.read().await;
            state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?
                .clone()
        };

        // Kill project shell tmux session if it exists
        if let Some(ref shell_name) = project.shell_tmux_session_name {
            let _ = self.tmux.kill_session(shell_name).await;
        }

        // Kill all sessions' tmux processes
        {
            let state = self.store.read().await;
            for session_id in &project.worktrees {
                if let Some(session) = state.get_session(session_id) {
                    if session.status.is_active()
                        && let Err(e) = self.tmux.kill_session(&session.tmux_session_name).await
                    {
                        warn!("Failed to kill tmux session: {}", e);
                    }
                    if let Some(ref shell_name) = session.shell_tmux_session_name {
                        let _ = self.tmux.kill_session(shell_name).await;
                    }
                }
            }
        }

        // Remove project from state (also removes sessions)
        let pid = *project_id;
        self.store
            .mutate(move |state| {
                state.remove_project(&pid);
            })
            .await?;

        info!("Removed project {}", project_id);
        Ok(())
    }
}
