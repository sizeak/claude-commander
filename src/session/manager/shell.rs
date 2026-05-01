//! Per-session shell operations: create, attach, and capture shell tmux sessions.

use super::*;

impl SessionManager {
    /// Ensure a shell tmux session exists for the given session (lazy creation)
    pub async fn ensure_shell_session(&self, session_id: &SessionId) -> Result<String> {
        let (existing_shell_name, tmux_name, worktree_path, project_id, status_bar) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (
                session.shell_tmux_session_name.clone(),
                session.tmux_session_name.clone(),
                session.worktree_path.clone(),
                session.project_id,
                self.status_bar_info(session, &state),
            )
        };
        let (tmux, _) = self.ops_for(&project_id).await?;

        let mut status_bar = status_bar;
        status_bar.is_shell = true;

        if let Some(ref shell_name) = existing_shell_name
            && tmux.session_exists(shell_name).await.unwrap_or(false)
        {
            tmux.configure_status_bar(shell_name, &status_bar).await;
            return Ok(shell_name.clone());
        }

        let shell_name = format!("{}-sh", tmux_name);

        if tmux.session_exists(&shell_name).await.unwrap_or(false) {
            let pane_dead = tmux.is_pane_dead(&shell_name).await.unwrap_or(false);
            if pane_dead {
                info!(
                    "Shell session {} has dead pane, killing for recreation",
                    shell_name
                );
                let _ = tmux.kill_session(&shell_name).await;
            } else {
                info!("Reusing existing shell session {}", shell_name);
                tmux.configure_status_bar(&shell_name, &status_bar).await;
                let sid = *session_id;
                let name = shell_name.clone();
                self.store
                    .mutate(move |state| {
                        if let Some(session) = state.get_session_mut(&sid) {
                            session.shell_tmux_session_name = Some(name);
                        }
                    })
                    .await?;
                return Ok(shell_name);
            }
        }

        let shell_program = self.config_store.read().shell_program.clone();
        tmux.create_session(&shell_name, &worktree_path, Some(&shell_program))
            .await?;
        tmux.configure_status_bar(&shell_name, &status_bar).await;

        // Store in session state
        let sid = *session_id;
        let name = shell_name.clone();
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.shell_tmux_session_name = Some(name);
                }
            })
            .await?;

        info!(
            "Created shell session {} for session {}",
            shell_name, session_id
        );
        Ok(shell_name)
    }

    /// Get attach command for the shell session (creates it if needed)
    pub async fn get_shell_attach_command(&self, session_id: &SessionId) -> Result<String> {
        let shell_name = self.ensure_shell_session(session_id).await?;
        let project_id = {
            let state = self.store.read().await;
            state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?
                .project_id
        };
        let (tmux, _) = self.ops_for(&project_id).await?;

        let exists = tmux.session_exists(&shell_name).await?;
        if !exists {
            let sid = *session_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(session) = state.get_session_mut(&sid) {
                        session.shell_tmux_session_name = None;
                    }
                })
                .await;
            return Err(SessionError::TmuxSessionNotFound(shell_name).into());
        }

        let pane_dead = tmux.is_pane_dead(&shell_name).await.unwrap_or(false);
        if pane_dead {
            let _ = tmux.kill_session(&shell_name).await;
            let sid = *session_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(session) = state.get_session_mut(&sid) {
                        session.shell_tmux_session_name = None;
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

    /// Get captured content for the shell session
    pub async fn get_shell_content(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<CapturedContent>> {
        let (shell_name, project_id) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (session.shell_tmux_session_name.clone(), session.project_id)
        };

        let shell_name = match shell_name {
            Some(name) => name,
            None => return Ok(None),
        };

        let (tmux, _) = self.ops_for(&project_id).await?;
        if !tmux.session_exists(&shell_name).await.unwrap_or(false) {
            let sid = *session_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(session) = state.get_session_mut(&sid) {
                        session.shell_tmux_session_name = None;
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
}
