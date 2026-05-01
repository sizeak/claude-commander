//! Session content retrieval: attach commands, captured content, and diffs.

use super::*;

impl SessionManager {
    /// Attach to a session (returns tmux session name for external attach).
    /// Auto-recreates the tmux session if it's missing or its pane has
    /// exited, honouring `config.resume_session` for the relaunch.
    pub async fn get_attach_command(&self, session_id: &SessionId) -> Result<String> {
        info!("get_attach_command called for session: {}", session_id);

        let (tmux_name, worktree_path, program, project_id, status_bar) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;

            info!(
                "Session found, status: {:?}, can_attach: {}",
                session.status,
                session.status.can_attach()
            );

            if !session.status.can_attach() {
                return Err(SessionError::InvalidState(*session_id).into());
            }

            (
                session.tmux_session_name.clone(),
                session.worktree_path.clone(),
                session.program.clone(),
                session.project_id,
                self.status_bar_info(session, &state),
            )
        };

        let (tmux, _) = self.ops_for(&project_id).await?;

        info!("Checking if tmux session '{}' exists", tmux_name);

        let exists = tmux.session_exists(&tmux_name).await?;
        info!("Tmux session exists: {}", exists);

        let needs_recreate = if !exists {
            info!("Tmux session not found, will recreate");
            true
        } else {
            let pane_dead = tmux.is_pane_dead(&tmux_name).await.unwrap_or(false);
            info!("Pane dead: {}", pane_dead);
            if pane_dead {
                info!("Pane is dead, killing tmux session for recreation");
                let _ = tmux.kill_session(&tmux_name).await;
                true
            } else {
                false
            }
        };

        if needs_recreate {
            let resume_program = if self.config_store.read().resume_session {
                format!("{} --resume", program)
            } else {
                program.clone()
            };
            info!("Recreating tmux session with: {}", resume_program);
            tmux.create_session(&tmux_name, &worktree_path, Some(&resume_program))
                .await?;
            tmux.configure_status_bar(&tmux_name, &status_bar).await;

            let sid = *session_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(session) = state.get_session_mut(&sid) {
                        session.set_status(SessionStatus::Running);
                    }
                })
                .await;
        }

        let cmd = format!("tmux attach-session -t {}", tmux_name);
        info!("Returning attach command: {}", cmd);
        Ok(cmd)
    }

    /// Force-kill the session's tmux pane and re-launch the program WITHOUT
    /// `--resume`. Used by the in-pane "restart fresh" shortcut so the user
    /// can break out of a `claude --resume` loop (e.g. when the previous
    /// session never produced anything to resume against).
    pub async fn restart_session_fresh(&self, session_id: &SessionId) -> Result<String> {
        let (tmux_name, worktree_path, program, project_id, status_bar) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            if !session.status.can_attach() {
                return Err(SessionError::InvalidState(*session_id).into());
            }
            (
                session.tmux_session_name.clone(),
                session.worktree_path.clone(),
                session.program.clone(),
                session.project_id,
                self.status_bar_info(session, &state),
            )
        };

        let (tmux, _) = self.ops_for(&project_id).await?;

        // Kill the existing tmux session if it's still around — `create_session`
        // refuses to clobber an existing one.
        if tmux.session_exists(&tmux_name).await.unwrap_or(false) {
            let _ = tmux.kill_session(&tmux_name).await;
        }

        info!("Restart-fresh: relaunching tmux session with: {}", program);
        tmux.create_session(&tmux_name, &worktree_path, Some(&program))
            .await?;
        tmux.configure_status_bar(&tmux_name, &status_bar).await;

        let sid = *session_id;
        let _ = self
            .store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.set_status(SessionStatus::Running);
                }
            })
            .await;

        Ok(format!("tmux attach-session -t {}", tmux_name))
    }

    /// Get captured content for a session
    pub async fn get_content(&self, session_id: &SessionId) -> Result<CapturedContent> {
        let (tmux_session_name, project_id) = {
            let state = self.store.read().await;
            let s = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (s.tmux_session_name.clone(), s.project_id)
        };
        let (tmux, _git) = self.ops_for(&project_id).await?;
        self.content_capture
            .get_content(&*tmux, &tmux_session_name)
            .await
    }

    /// Get diff for a session
    pub async fn get_diff(&self, session_id: &SessionId) -> Result<Arc<DiffInfo>> {
        let (worktree_path, project_id) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (session.worktree_path.clone(), session.project_id)
        };
        let (_tmux, git) = self.ops_for(&project_id).await?;
        self.diff_cache
            .get_diff(session_id, &*git, &worktree_path)
            .await
    }
}
