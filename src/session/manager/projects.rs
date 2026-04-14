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
