//! Session manager - coordinates session lifecycle
//!
//! Handles the creation, pause, resume, and termination of sessions,
//! coordinating between tmux and git operations.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, instrument, warn};

use crate::config::{AppState, Config};
use crate::error::{Result, SessionError};
use crate::git::{DiffCache, DiffInfo, GitBackend, WorktreeManager};
use crate::session::{
    AgentState, Project, ProjectId, SessionId, SessionStatus, WorktreeSession,
};
use crate::tmux::{CapturedContent, ContentCapture, StateDetector, TmuxExecutor};

/// Session manager coordinates all session operations
pub struct SessionManager {
    /// Application configuration
    config: Config,
    /// Persistent state
    pub app_state: Arc<RwLock<AppState>>,
    /// Tmux executor
    pub tmux: TmuxExecutor,
    /// Content capture cache
    content_capture: ContentCapture,
    /// State detector
    state_detector: StateDetector,
    /// Diff cache
    diff_cache: DiffCache,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(config: Config, state: Arc<RwLock<AppState>>) -> Self {
        let tmux = TmuxExecutor::with_max_concurrent(config.max_concurrent_tmux);
        let content_capture = ContentCapture::with_ttl(
            tmux.clone(),
            std::time::Duration::from_millis(config.capture_cache_ttl_ms),
        );
        let diff_cache =
            DiffCache::with_ttl(std::time::Duration::from_millis(config.diff_cache_ttl_ms));
        let state_detector = StateDetector::new();

        Self {
            config,
            app_state: state,
            tmux,
            content_capture,
            state_detector,
            diff_cache,
        }
    }

    /// Check if tmux is available
    pub async fn check_tmux(&self) -> Result<()> {
        self.tmux.check_installed().await
    }

    /// Add a new project (git repository)
    #[instrument(skip(self))]
    pub async fn add_project(&self, repo_path: PathBuf) -> Result<ProjectId> {
        // Discover git repository
        let backend = GitBackend::discover(&repo_path)?;
        let main_branch = backend.detect_main_branch()?;
        let name = backend.repo_name();

        info!("Adding project '{}' from {:?}", name, repo_path);

        let project = Project::new(name, backend.path().to_path_buf(), main_branch);
        let project_id = project.id;

        // Save to state
        {
            let mut state = self.app_state.write().await;
            state.add_project(project);
            state.save()?;
        }

        Ok(project_id)
    }

    /// Remove a project and all its sessions
    #[instrument(skip(self))]
    pub async fn remove_project(&self, project_id: &ProjectId) -> Result<()> {
        let mut state = self.app_state.write().await;

        // Get project first to find sessions
        let project = state
            .get_project(project_id)
            .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?
            .clone();

        // Kill all sessions
        for session_id in &project.worktrees {
            if let Some(session) = state.get_session(session_id) {
                // Kill tmux session if running
                if session.status.is_active() {
                    if let Err(e) = self.tmux.kill_session(&session.tmux_session_name).await {
                        warn!("Failed to kill tmux session: {}", e);
                    }
                }
            }
        }

        // Remove project (also removes sessions)
        state.remove_project(project_id);
        state.save()?;

        info!("Removed project {}", project_id);
        Ok(())
    }

    /// Create a new worktree session
    #[instrument(skip(self))]
    pub async fn create_session(
        &self,
        project_id: &ProjectId,
        title: String,
        program: Option<String>,
    ) -> Result<SessionId> {
        let program = program.unwrap_or_else(|| self.config.default_program.clone());

        // Get project info
        let (repo_path, _main_branch) = {
            let state = self.app_state.read().await;
            let project = state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
            (project.repo_path.clone(), project.main_branch.clone())
        };

        // Generate branch name from title
        let branch_name = self.generate_branch_name(&title);

        info!(
            "Creating session '{}' with branch '{}' in project {}",
            title, branch_name, project_id
        );

        // Create git backend and worktree manager
        let backend = GitBackend::open(&repo_path)?;
        let worktrees_dir = self.config.worktrees_dir()?;
        let worktree_manager = WorktreeManager::new(backend, worktrees_dir);

        // Generate unique worktree name
        let worktree_name = format!("{}-{}", self.sanitize_name(&title), uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or(""));

        // Create worktree
        let worktree_info = worktree_manager
            .create_worktree(&worktree_name, &branch_name)
            .await?;

        // Create session object
        let mut session = WorktreeSession::new(
            *project_id,
            title,
            branch_name,
            worktree_info.path.clone(),
            program.clone(),
        );
        session.base_commit = Some(worktree_info.head);
        let session_id = session.id;
        let tmux_session_name = session.tmux_session_name.clone();

        // Create tmux session in the worktree directory
        self.tmux
            .create_session(&tmux_session_name, &worktree_info.path, Some(&program))
            .await?;

        // Save session to state
        {
            let mut state = self.app_state.write().await;
            state.add_session(session);
            state.save()?;
        }

        info!("Created session {} with tmux session {}", session_id, tmux_session_name);
        Ok(session_id)
    }

    /// Pause a session (detach from tmux, keep worktree)
    #[instrument(skip(self))]
    pub async fn pause_session(&self, session_id: &SessionId) -> Result<()> {
        let mut state = self.app_state.write().await;

        let session = state
            .get_session_mut(session_id)
            .ok_or(SessionError::NotFound(*session_id))?;

        if !session.status.can_pause() {
            return Err(SessionError::InvalidState(*session_id).into());
        }

        // Update status
        session.set_status(SessionStatus::Paused);
        state.save()?;

        info!("Paused session {}", session_id);
        Ok(())
    }

    /// Resume a paused session
    #[instrument(skip(self))]
    pub async fn resume_session(&self, session_id: &SessionId) -> Result<()> {
        let mut state = self.app_state.write().await;

        let session = state
            .get_session_mut(session_id)
            .ok_or(SessionError::NotFound(*session_id))?;

        if !session.status.can_resume() {
            return Err(SessionError::InvalidState(*session_id).into());
        }

        // Check if tmux session still exists
        let tmux_session_name = session.tmux_session_name.clone();
        let exists = self.tmux.session_exists(&tmux_session_name).await?;

        if !exists {
            // Recreate tmux session
            let worktree_path = session.worktree_path.clone();
            let program = session.program.clone();
            self.tmux
                .create_session(&tmux_session_name, &worktree_path, Some(&program))
                .await?;
        }

        // Update status
        session.set_status(SessionStatus::Running);
        state.save()?;

        info!("Resumed session {}", session_id);
        Ok(())
    }

    /// Kill a session (stop tmux, optionally remove worktree)
    #[instrument(skip(self))]
    pub async fn kill_session(&self, session_id: &SessionId, remove_worktree: bool) -> Result<()> {
        let session = {
            let state = self.app_state.read().await;
            state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?
                .clone()
        };

        // Kill tmux session
        if let Err(e) = self.tmux.kill_session(&session.tmux_session_name).await {
            warn!("Failed to kill tmux session: {}", e);
        }

        // Optionally remove worktree
        if remove_worktree {
            let repo_path = {
                let state = self.app_state.read().await;
                state
                    .get_project(&session.project_id)
                    .map(|p| p.repo_path.clone())
            };

            if let Some(repo_path) = repo_path {
                if let Ok(backend) = GitBackend::open(&repo_path) {
                    let worktree_manager =
                        WorktreeManager::new(backend, self.config.worktrees_dir()?);
                    if let Err(e) = worktree_manager
                        .remove_worktree(&session.worktree_path, true)
                        .await
                    {
                        warn!("Failed to remove worktree: {}", e);
                    }
                }
            }
        }

        // Update state
        {
            let mut state = self.app_state.write().await;
            if let Some(session) = state.get_session_mut(session_id) {
                session.set_status(SessionStatus::Stopped);
            }
            state.save()?;
        }

        info!("Killed session {}", session_id);
        Ok(())
    }

    /// Delete a session (remove from state)
    #[instrument(skip(self))]
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        // First kill if active
        {
            let state = self.app_state.read().await;
            if let Some(session) = state.get_session(session_id) {
                if session.status.is_active() {
                    drop(state);
                    self.kill_session(session_id, true).await?;
                }
            }
        }

        // Remove from state
        {
            let mut state = self.app_state.write().await;
            state.remove_session(session_id);
            state.save()?;
        }

        info!("Deleted session {}", session_id);
        Ok(())
    }

    /// Attach to a session (returns tmux session name for external attach)
    pub async fn get_attach_command(&self, session_id: &SessionId) -> Result<String> {
        info!("get_attach_command called for session: {}", session_id);

        let tmux_name = {
            let state = self.app_state.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;

            info!("Session found, status: {:?}, can_attach: {}", session.status, session.status.can_attach());

            if !session.status.can_attach() {
                return Err(SessionError::InvalidState(*session_id).into());
            }

            session.tmux_session_name.clone()
        };

        info!("Checking if tmux session '{}' exists", tmux_name);

        // Verify tmux session actually exists before returning attach command
        let exists = self.tmux.session_exists(&tmux_name).await?;
        info!("Tmux session exists: {}", exists);

        if !exists {
            // Update state to reflect that session is stopped
            info!("Tmux session not found, updating state to Stopped");
            let mut state = self.app_state.write().await;
            if let Some(session) = state.get_session_mut(session_id) {
                session.set_status(SessionStatus::Stopped);
                let _ = state.save();
            }
            return Err(SessionError::TmuxSessionNotFound(tmux_name).into());
        }

        // Check if the pane is dead (program exited)
        let pane_dead = self.tmux.is_pane_dead(&tmux_name).await.unwrap_or(false);
        info!("Pane dead: {}", pane_dead);

        if pane_dead {
            // The program inside tmux has exited - kill the session and update state
            info!("Pane is dead, killing tmux session and updating state");
            let _ = self.tmux.kill_session(&tmux_name).await;
            let mut state = self.app_state.write().await;
            if let Some(session) = state.get_session_mut(session_id) {
                session.set_status(SessionStatus::Stopped);
                let _ = state.save();
            }
            return Err(SessionError::TmuxSessionNotFound(
                format!("{} (program exited)", tmux_name)
            ).into());
        }

        let cmd = format!("tmux attach-session -t {}", tmux_name);
        info!("Returning attach command: {}", cmd);
        Ok(cmd)
    }

    /// Get captured content for a session
    pub async fn get_content(&self, session_id: &SessionId) -> Result<CapturedContent> {
        let tmux_session_name = {
            let state = self.app_state.read().await;
            state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?
                .tmux_session_name
                .clone()
        };

        self.content_capture
            .get_content(session_id, &tmux_session_name)
            .await
    }

    /// Detect agent state for a session
    pub async fn detect_agent_state(&self, session_id: &SessionId) -> Result<AgentState> {
        let content = self.get_content(session_id).await?;
        Ok(self.state_detector.detect(&content))
    }

    /// Get diff for a session
    pub async fn get_diff(&self, session_id: &SessionId) -> Result<DiffInfo> {
        let worktree_path = {
            let state = self.app_state.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            session.worktree_path.clone()
        };

        self.diff_cache
            .get_diff(session_id, &worktree_path)
            .await
    }

    /// Update agent state for all active sessions
    pub async fn update_all_states(&self) -> Result<()> {
        let session_ids: Vec<SessionId> = {
            let state = self.app_state.read().await;
            state
                .get_active_sessions()
                .iter()
                .map(|s| s.id)
                .collect()
        };

        for session_id in session_ids {
            if let Ok(agent_state) = self.detect_agent_state(&session_id).await {
                let mut state = self.app_state.write().await;
                if let Some(session) = state.get_session_mut(&session_id) {
                    session.set_agent_state(agent_state);
                }
            }
        }

        Ok(())
    }

    /// Generate branch name from title
    fn generate_branch_name(&self, title: &str) -> String {
        let sanitized = self.sanitize_name(title);

        if self.config.branch_prefix.is_empty() {
            sanitized
        } else {
            format!("{}/{}", self.config.branch_prefix, sanitized)
        }
    }

    /// Sanitize a name for use as branch/directory name
    fn sanitize_name(&self, name: &str) -> String {
        name.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_name() {
        let config = Config::default();
        let state = Arc::new(RwLock::new(AppState::new()));
        let manager = SessionManager::new(config, state);

        assert_eq!(manager.sanitize_name("Hello World"), "hello-world");
        assert_eq!(manager.sanitize_name("Feature/Auth"), "feature-auth");
        assert_eq!(manager.sanitize_name("--test--"), "test");
    }

    #[test]
    fn test_generate_branch_name() {
        let mut config = Config::default();
        let state = Arc::new(RwLock::new(AppState::new()));

        // Without prefix
        let manager = SessionManager::new(config.clone(), state.clone());
        assert_eq!(manager.generate_branch_name("Feature Auth"), "feature-auth");

        // With prefix
        config.branch_prefix = "cc".to_string();
        let manager = SessionManager::new(config, state);
        assert_eq!(manager.generate_branch_name("Feature Auth"), "cc/feature-auth");
    }
}
