//! Per-project shell operations: create, attach, capture, and diff for project shells.

use super::*;

impl SessionManager {
    /// Ensure a shell tmux session exists for the given project (lazy creation)
    pub async fn ensure_project_shell_session(&self, project_id: &ProjectId) -> Result<String> {
        let (existing_shell_name, repo_path, id_prefix) = {
            let state = self.store.read().await;
            let project = state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
            (
                project.shell_tmux_session_name.clone(),
                project.repo_path.clone(),
                project_id.to_string(),
            )
        };
        let (tmux, _) = self.ops_for(project_id).await?;

        if let Some(ref shell_name) = existing_shell_name
            && tmux.session_exists(shell_name).await.unwrap_or(false)
        {
            return Ok(shell_name.clone());
        }

        let shell_name = format!("cc-proj-{}-sh", id_prefix);

        if tmux.session_exists(&shell_name).await.unwrap_or(false) {
            let pane_dead = tmux.is_pane_dead(&shell_name).await.unwrap_or(false);
            if pane_dead {
                info!(
                    "Project shell session {} has dead pane, killing for recreation",
                    shell_name
                );
                let _ = tmux.kill_session(&shell_name).await;
            } else {
                info!("Reusing existing project shell session {}", shell_name);
                let pid = *project_id;
                let name = shell_name.clone();
                self.store
                    .mutate(move |state| {
                        if let Some(project) = state.get_project_mut(&pid) {
                            project.shell_tmux_session_name = Some(name);
                        }
                    })
                    .await?;
                return Ok(shell_name);
            }
        }

        let shell_program = self.config_store.read().shell_program.clone();
        tmux.create_session(&shell_name, &repo_path, Some(&shell_program))
            .await?;

        // Store in project state
        let pid = *project_id;
        let name = shell_name.clone();
        self.store
            .mutate(move |state| {
                if let Some(project) = state.get_project_mut(&pid) {
                    project.shell_tmux_session_name = Some(name);
                }
            })
            .await?;

        info!(
            "Created shell session {} for project {}",
            shell_name, project_id
        );
        Ok(shell_name)
    }

    /// Get attach command for the project shell session (creates it if needed)
    pub async fn get_project_shell_attach_command(&self, project_id: &ProjectId) -> Result<String> {
        let shell_name = self.ensure_project_shell_session(project_id).await?;
        let (tmux, _) = self.ops_for(project_id).await?;

        let exists = tmux.session_exists(&shell_name).await?;
        if !exists {
            let pid = *project_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(project) = state.get_project_mut(&pid) {
                        project.shell_tmux_session_name = None;
                    }
                })
                .await;
            return Err(SessionError::TmuxSessionNotFound(shell_name).into());
        }

        let pane_dead = tmux.is_pane_dead(&shell_name).await.unwrap_or(false);
        if pane_dead {
            let _ = tmux.kill_session(&shell_name).await;
            let pid = *project_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(project) = state.get_project_mut(&pid) {
                        project.shell_tmux_session_name = None;
                    }
                })
                .await;
            return Err(SessionError::TmuxSessionNotFound(format!(
                "{} (shell exited)",
                shell_name
            ))
            .into());
        }

        Ok(format!("tmux attach-session -t {}", shell_name))
    }

    /// Get captured content for the project shell session
    pub async fn get_project_shell_content(
        &self,
        project_id: &ProjectId,
    ) -> Result<Option<CapturedContent>> {
        let shell_name = {
            let state = self.store.read().await;
            let project = state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
            project.shell_tmux_session_name.clone()
        };

        let shell_name = match shell_name {
            Some(name) => name,
            None => return Ok(None),
        };

        let (tmux, _) = self.ops_for(project_id).await?;
        if !tmux.session_exists(&shell_name).await.unwrap_or(false) {
            let pid = *project_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(project) = state.get_project_mut(&pid) {
                        project.shell_tmux_session_name = None;
                    }
                })
                .await;
            return Ok(None);
        }

        match self.content_capture.get_content(&*tmux, &shell_name).await {
            Ok(content) => Ok(Some(content)),
            Err(_) => Ok(None),
        }
    }

    /// Get diff for a project (uncommitted changes in repo)
    pub async fn get_project_diff(&self, project_id: &ProjectId) -> Result<Arc<DiffInfo>> {
        let repo_path = {
            let state = self.store.read().await;
            let project = state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
            project.repo_path.clone()
        };
        let (_tmux, git) = self.ops_for(project_id).await?;
        self.project_diff_cache
            .get_diff(project_id, &*git, &repo_path)
            .await
    }
}
