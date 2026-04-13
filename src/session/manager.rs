//! Session manager - coordinates session lifecycle
//!
//! Handles the creation, restart, and termination of sessions,
//! coordinating between tmux and git operations.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, instrument, warn};

use crate::config::{AppState, ConfigStore, StateStore};
use crate::error::{Result, SessionError};
use crate::git::{DiffCache, DiffInfo, GitBackend, WorktreeManager};
use crate::session::{Project, ProjectId, SessionId, SessionStatus, WorktreeSession};
use crate::tmux::{CapturedContent, ContentCapture, StatusBarInfo, TmuxExecutor};

/// Session manager coordinates all session operations
pub struct SessionManager {
    /// Shared configuration store (hot-reloaded)
    config_store: Arc<ConfigStore>,
    /// Concurrent-safe persistent state store
    pub store: Arc<StateStore>,
    /// Tmux executor
    pub tmux: TmuxExecutor,
    /// Content capture cache
    content_capture: ContentCapture,
    /// Diff cache for sessions
    diff_cache: DiffCache<SessionId>,
    /// Diff cache for projects
    project_diff_cache: DiffCache<ProjectId>,
    /// Tmux status-style string derived from theme
    tmux_status_style: String,
}

impl Clone for SessionManager {
    fn clone(&self) -> Self {
        Self {
            config_store: self.config_store.clone(),
            store: self.store.clone(),
            tmux: self.tmux.clone(),
            content_capture: self.content_capture.clone(),
            diff_cache: self.diff_cache.clone(),
            project_diff_cache: self.project_diff_cache.clone(),
            tmux_status_style: self.tmux_status_style.clone(),
        }
    }
}

impl SessionManager {
    /// Create a new session manager
    ///
    /// Note: `max_concurrent_tmux`, `capture_cache_ttl_ms`, and `diff_cache_ttl_ms`
    /// are read from the config at construction time and are **not** hot-reloaded.
    pub fn new(
        config_store: Arc<ConfigStore>,
        store: Arc<StateStore>,
        tmux_status_style: impl Into<String>,
    ) -> Self {
        let config = config_store.read();
        let tmux = TmuxExecutor::with_max_concurrent(config.max_concurrent_tmux);
        let content_capture = ContentCapture::with_ttl(
            tmux.clone(),
            std::time::Duration::from_millis(config.capture_cache_ttl_ms),
        );
        let diff_cache =
            DiffCache::with_ttl(std::time::Duration::from_millis(config.diff_cache_ttl_ms));
        let project_diff_cache =
            DiffCache::with_ttl(std::time::Duration::from_millis(config.diff_cache_ttl_ms));
        drop(config);

        Self {
            config_store,
            store,
            tmux,
            content_capture,
            diff_cache,
            project_diff_cache,
            tmux_status_style: tmux_status_style.into(),
        }
    }

    /// Check if tmux is available
    pub async fn check_tmux(&self) -> Result<()> {
        self.tmux.check_installed().await
    }

    /// Build a `StatusBarInfo` from session metadata
    pub fn status_bar_info(&self, session: &WorktreeSession, state: &AppState) -> StatusBarInfo {
        let project_name = state
            .get_project(&session.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        StatusBarInfo {
            branch: session.branch.clone(),
            pr_number: session.pr_number,
            pr_merged: session.pr_merged,
            status_style: self.tmux_status_style.clone(),
            is_shell: false,
            project_name,
        }
    }

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

    /// Prepare a placeholder session in `Creating` state.
    ///
    /// This inserts the session into state immediately so the UI can show a
    /// spinner. Call `finalize_session` in a background task to do the heavy
    /// git/tmux work.
    #[instrument(skip(self))]
    pub async fn prepare_session(
        &self,
        project_id: &ProjectId,
        title: String,
        program: Option<String>,
    ) -> Result<SessionId> {
        let program = program.unwrap_or_else(|| self.config_store.read().default_program.clone());

        // Validate project exists
        {
            let state = self.store.read().await;
            state
                .get_project(project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
        }

        let branch_name = self.generate_branch_name(&title);

        let session = WorktreeSession::new_creating(*project_id, title, branch_name, program);
        let session_id = session.id;

        self.store
            .mutate(move |state| {
                state.add_session(session);
            })
            .await?;

        info!("Prepared creating session {}", session_id);
        Ok(session_id)
    }

    /// Finalize a session that was created with `prepare_session`.
    ///
    /// Performs the heavy work: git fetch, worktree creation, tmux session
    /// setup. On success, transitions the session from `Creating` to `Running`.
    #[instrument(skip(self))]
    pub async fn finalize_session(&self, session_id: &SessionId) -> Result<SessionId> {
        // Read session and project info
        let (project_id, title, branch_name, program) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (
                session.project_id,
                session.title.clone(),
                session.branch.clone(),
                session.program.clone(),
            )
        };

        let (repo_path, main_branch) = {
            let state = self.store.read().await;
            let project = state
                .get_project(&project_id)
                .ok_or_else(|| SessionError::ProjectNotFound(project_id.to_string()))?;
            (project.repo_path.clone(), project.main_branch.clone())
        };

        info!(
            "Finalizing session '{}' with branch '{}' in project {}",
            title, branch_name, project_id
        );

        // Fetch latest changes from origin
        if self.config_store.read().fetch_before_create {
            info!(
                "Fetching latest changes from origin in {}",
                repo_path.display()
            );
            let output = tokio::process::Command::new("git")
                .current_dir(&repo_path)
                .args(["fetch", "origin"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("git fetch failed (continuing anyway): {}", stderr);
            }
        }

        // Generate unique worktree name
        let worktree_name = format!(
            "{}-{}",
            self.sanitize_name(&title),
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("")
        );

        // Create worktree — sync gix work (branch check + start point) is done
        // in a block so non-Sync types are dropped before the first .await,
        // keeping the overall future Send.
        let worktrees_dir = self.config_store.read().worktrees_dir()?;
        let (branch_exists, start_point) = {
            let backend = GitBackend::open(&repo_path)?;
            let exists = backend.branch_exists(&branch_name)?;
            let remote_ref = format!("refs/remotes/origin/{}", main_branch);
            let sp = if backend.ref_exists(&remote_ref)? {
                Some(format!("origin/{}", main_branch))
            } else {
                None
            };
            (exists, sp)
        };
        let worktree_path = worktrees_dir.join(&worktree_name);
        let worktree_info = WorktreeManager::run_create_worktree(
            worktrees_dir,
            repo_path.clone(),
            worktree_path,
            branch_name.clone(),
            branch_exists,
            start_point,
        )
        .await?;

        // Read tmux_session_name from the placeholder session
        let tmux_session_name = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            session.tmux_session_name.clone()
        };

        // Create tmux session in the worktree directory
        self.tmux
            .create_session(&tmux_session_name, &worktree_info.path, Some(&program))
            .await?;

        // Update session to Running with the real worktree info
        let sid = *session_id;
        let wt_path = worktree_info.path.clone();
        let head = worktree_info.head.clone();
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.worktree_path = wt_path;
                    session.base_commit = Some(head);
                    session.set_status(SessionStatus::Running);
                }
            })
            .await?;

        // Configure CC status bar (branch only, no PR yet)
        let status_bar = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            self.status_bar_info(session, &state)
        };
        self.tmux
            .configure_status_bar(&tmux_session_name, &status_bar)
            .await;

        info!(
            "Finalized session {} with tmux session {}",
            session_id, tmux_session_name
        );
        Ok(*session_id)
    }

    /// Remove a session that is still in `Creating` state (e.g., on failure or startup cleanup).
    #[instrument(skip(self))]
    pub async fn remove_creating_session(&self, session_id: &SessionId) -> Result<()> {
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                state.remove_session(&sid);
            })
            .await?;
        info!("Removed creating session {}", session_id);
        Ok(())
    }

    /// Kill tmux sessions (main + shell) for a worktree session.
    async fn kill_tmux_sessions(&self, tmux_name: &str, shell_tmux_name: Option<&str>) {
        if let Err(e) = self.tmux.kill_session(tmux_name).await {
            warn!("Failed to kill tmux session: {}", e);
        }
        if let Some(shell_name) = shell_tmux_name {
            let _ = self.tmux.kill_session(shell_name).await;
        }
    }

    /// Restart a session (kill tmux and recreate with --resume)
    #[instrument(skip(self))]
    pub async fn restart_session(&self, session_id: &SessionId) -> Result<()> {
        let (tmux_session_name, shell_tmux_name, worktree_path, program, status_bar) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (
                session.tmux_session_name.clone(),
                session.shell_tmux_session_name.clone(),
                session.worktree_path.clone(),
                session.program.clone(),
                self.status_bar_info(session, &state),
            )
        };

        self.kill_tmux_sessions(&tmux_session_name, shell_tmux_name.as_deref())
            .await;

        // Create a fresh tmux session with --resume
        let resume_program = format!("{} --resume", program);
        let create_result = self
            .tmux
            .create_session(&tmux_session_name, &worktree_path, Some(&resume_program))
            .await;

        if let Err(e) = create_result {
            // Tmux is dead but recreation failed — mark as Stopped so state is consistent
            let sid = *session_id;
            let _ = self
                .store
                .mutate(move |state| {
                    if let Some(session) = state.get_session_mut(&sid) {
                        session.set_status(SessionStatus::Stopped);
                    }
                })
                .await;
            return Err(e);
        }

        // Configure status bar on the new session
        self.tmux
            .configure_status_bar(&tmux_session_name, &status_bar)
            .await;

        // Set status to Running
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.set_status(SessionStatus::Running);
                }
            })
            .await?;

        info!("Restarted session {}", session_id);
        Ok(())
    }

    /// Kill a session (stop tmux, optionally remove worktree)
    #[instrument(skip(self))]
    pub async fn kill_session(&self, session_id: &SessionId, remove_worktree: bool) -> Result<()> {
        let session = {
            let state = self.store.read().await;
            state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?
                .clone()
        };

        self.kill_tmux_sessions(
            &session.tmux_session_name,
            session.shell_tmux_session_name.as_deref(),
        )
        .await;

        // Optionally remove worktree
        if remove_worktree {
            let repo_path = {
                let state = self.store.read().await;
                state
                    .get_project(&session.project_id)
                    .map(|p| p.repo_path.clone())
            };

            if let Some(repo_path) = repo_path
                && let Ok(backend) = GitBackend::open(&repo_path)
            {
                let worktree_manager =
                    WorktreeManager::new(backend, self.config_store.read().worktrees_dir()?);
                if let Err(e) = worktree_manager
                    .remove_worktree(&session.worktree_path, true)
                    .await
                {
                    warn!("Failed to remove worktree: {}", e);
                }
            }
        }

        // Update state
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                if let Some(session) = state.get_session_mut(&sid) {
                    session.set_status(SessionStatus::Stopped);
                }
            })
            .await?;

        info!("Killed session {}", session_id);
        Ok(())
    }

    /// Delete a session (remove from state)
    #[instrument(skip(self))]
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        // First kill if active
        {
            let state = self.store.read().await;
            if let Some(session) = state.get_session(session_id)
                && session.status.is_active()
            {
                drop(state);
                self.kill_session(session_id, true).await?;
            }
        }

        // Remove from state
        let sid = *session_id;
        self.store
            .mutate(move |state| {
                state.remove_session(&sid);
            })
            .await?;

        info!("Deleted session {}", session_id);
        Ok(())
    }

    /// Attach to a session (returns tmux session name for external attach)
    pub async fn get_attach_command(&self, session_id: &SessionId) -> Result<String> {
        info!("get_attach_command called for session: {}", session_id);

        let (tmux_name, worktree_path, program, status_bar) = {
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
                self.status_bar_info(session, &state),
            )
        };

        info!("Checking if tmux session '{}' exists", tmux_name);

        // Verify tmux session actually exists before returning attach command
        let exists = self.tmux.session_exists(&tmux_name).await?;
        info!("Tmux session exists: {}", exists);

        let needs_recreate = if !exists {
            info!("Tmux session not found, will recreate");
            true
        } else {
            // Check if the pane is dead (program exited)
            let pane_dead = self.tmux.is_pane_dead(&tmux_name).await.unwrap_or(false);
            info!("Pane dead: {}", pane_dead);
            if pane_dead {
                info!("Pane is dead, killing tmux session for recreation");
                let _ = self.tmux.kill_session(&tmux_name).await;
                true
            } else {
                false
            }
        };

        if needs_recreate {
            // Recreate the tmux session with --resume so the agent picks up where it left off
            let resume_program = format!("{} --resume", program);
            info!("Recreating tmux session with: {}", resume_program);
            self.tmux
                .create_session(&tmux_name, &worktree_path, Some(&resume_program))
                .await?;

            // Configure CC status bar on the recreated session
            self.tmux
                .configure_status_bar(&tmux_name, &status_bar)
                .await;

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

    /// Ensure a shell tmux session exists for the given session (lazy creation)
    pub async fn ensure_shell_session(&self, session_id: &SessionId) -> Result<String> {
        let (existing_shell_name, tmux_name, worktree_path, status_bar) = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            (
                session.shell_tmux_session_name.clone(),
                session.tmux_session_name.clone(),
                session.worktree_path.clone(),
                self.status_bar_info(session, &state),
            )
        };

        let mut status_bar = status_bar;
        status_bar.is_shell = true;

        // If shell session already exists in tmux, ensure status bar and return
        if let Some(ref shell_name) = existing_shell_name
            && self.tmux.session_exists(shell_name).await.unwrap_or(false)
        {
            self.tmux
                .configure_status_bar(shell_name, &status_bar)
                .await;
            return Ok(shell_name.clone());
        }

        // Create new shell tmux session
        let shell_name = format!("{}-sh", tmux_name);

        // Check if a tmux session with this name already exists (stale state)
        if self.tmux.session_exists(&shell_name).await.unwrap_or(false) {
            let pane_dead = self.tmux.is_pane_dead(&shell_name).await.unwrap_or(false);
            if pane_dead {
                info!(
                    "Shell session {} has dead pane, killing for recreation",
                    shell_name
                );
                let _ = self.tmux.kill_session(&shell_name).await;
            } else {
                info!("Reusing existing shell session {}", shell_name);
                self.tmux
                    .configure_status_bar(&shell_name, &status_bar)
                    .await;
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
        self.tmux
            .create_session(&shell_name, &worktree_path, Some(&shell_program))
            .await?;

        // Configure CC status bar on the shell session
        self.tmux
            .configure_status_bar(&shell_name, &status_bar)
            .await;

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

        // Verify tmux session exists and pane is alive
        let exists = self.tmux.session_exists(&shell_name).await?;
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

        let pane_dead = self.tmux.is_pane_dead(&shell_name).await.unwrap_or(false);
        if pane_dead {
            let _ = self.tmux.kill_session(&shell_name).await;
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
        let shell_name = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            session.shell_tmux_session_name.clone()
        };

        let shell_name = match shell_name {
            Some(name) => name,
            None => return Ok(None),
        };

        // Check if tmux session still exists
        if !self.tmux.session_exists(&shell_name).await.unwrap_or(false) {
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

        match self.content_capture.get_content(&shell_name).await {
            Ok(content) => Ok(Some(content)),
            Err(_) => Ok(None),
        }
    }

    /// Get captured content for a session
    pub async fn get_content(&self, session_id: &SessionId) -> Result<CapturedContent> {
        let tmux_session_name = {
            let state = self.store.read().await;
            state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?
                .tmux_session_name
                .clone()
        };

        self.content_capture.get_content(&tmux_session_name).await
    }

    /// Get diff for a session
    pub async fn get_diff(&self, session_id: &SessionId) -> Result<Arc<DiffInfo>> {
        let worktree_path = {
            let state = self.store.read().await;
            let session = state
                .get_session(session_id)
                .ok_or(SessionError::NotFound(*session_id))?;
            session.worktree_path.clone()
        };

        self.diff_cache.get_diff(session_id, &worktree_path).await
    }

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

        // If shell session already exists in tmux, return its name
        if let Some(ref shell_name) = existing_shell_name
            && self.tmux.session_exists(shell_name).await.unwrap_or(false)
        {
            return Ok(shell_name.clone());
        }

        // Create new shell tmux session
        let shell_name = format!("cc-proj-{}-sh", id_prefix);

        // Check if a tmux session with this name already exists (stale state)
        if self.tmux.session_exists(&shell_name).await.unwrap_or(false) {
            let pane_dead = self.tmux.is_pane_dead(&shell_name).await.unwrap_or(false);
            if pane_dead {
                info!(
                    "Project shell session {} has dead pane, killing for recreation",
                    shell_name
                );
                let _ = self.tmux.kill_session(&shell_name).await;
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
        self.tmux
            .create_session(&shell_name, &repo_path, Some(&shell_program))
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

        // Verify tmux session exists and pane is alive
        let exists = self.tmux.session_exists(&shell_name).await?;
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

        let pane_dead = self.tmux.is_pane_dead(&shell_name).await.unwrap_or(false);
        if pane_dead {
            let _ = self.tmux.kill_session(&shell_name).await;
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

        // Check if tmux session still exists
        if !self.tmux.session_exists(&shell_name).await.unwrap_or(false) {
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

        match self.content_capture.get_content(&shell_name).await {
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

        self.project_diff_cache
            .get_diff(project_id, &repo_path)
            .await
    }

    /// Sync unmanaged git worktrees as stopped sessions
    ///
    /// Lists actual git worktrees for the project and imports any that aren't
    /// already tracked as sessions. Imported worktrees get `Stopped` status —
    /// they have no running tmux session but can be attached to (which will
    /// recreate the tmux session on demand).
    #[instrument(skip(self))]
    pub async fn sync_worktrees(&self, project_id: &ProjectId) -> Result<usize> {
        let (repo_path, existing_paths) = {
            let state = self.store.read().await;
            let project = match state.get_project(project_id) {
                Some(p) => p,
                None => return Ok(0),
            };

            let repo_path = project.repo_path.clone();

            // Collect canonicalized worktree paths from all existing sessions
            let paths: Vec<PathBuf> = project
                .worktrees
                .iter()
                .filter_map(|sid| state.get_session(sid))
                .filter_map(|s| std::fs::canonicalize(&s.worktree_path).ok())
                .collect();

            (repo_path, paths)
        };

        // Open git backend and list worktrees
        let backend = match GitBackend::open(&repo_path) {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to open git backend for sync: {}", e);
                return Ok(0);
            }
        };

        let worktrees_dir = self.config_store.read().worktrees_dir()?;
        let canonical_worktrees_dir =
            std::fs::canonicalize(&worktrees_dir).unwrap_or_else(|_| worktrees_dir.clone());
        let worktree_manager = WorktreeManager::new(backend, worktrees_dir);

        let worktrees = match worktree_manager.list_worktrees().await {
            Ok(w) => w,
            Err(e) => {
                warn!("Failed to list worktrees for sync: {}", e);
                return Ok(0);
            }
        };

        // Also canonicalize the repo path for main worktree comparison
        let canonical_repo = std::fs::canonicalize(&repo_path).unwrap_or(repo_path);

        let mut imported = 0;
        let mut new_sessions = Vec::new();

        for wt in &worktrees {
            if wt.is_main {
                continue;
            }

            let canonical_wt = match std::fs::canonicalize(&wt.path) {
                Ok(p) => p,
                Err(_) => continue, // Worktree path doesn't exist, skip
            };

            // Skip if this path matches the main repo
            if canonical_wt == canonical_repo {
                continue;
            }

            // Only import worktrees inside the managed worktrees directory
            if !canonical_wt.starts_with(&canonical_worktrees_dir) {
                continue;
            }

            // Skip if already tracked by an existing session
            if existing_paths.contains(&canonical_wt) {
                continue;
            }

            let mut session = WorktreeSession::new(
                *project_id,
                wt.branch.clone(),
                wt.branch.clone(),
                wt.path.clone(),
                self.config_store.read().default_program.clone(),
            );
            session.set_status(SessionStatus::Stopped);
            session.base_commit = Some(wt.head.clone());

            info!(
                "Importing unmanaged worktree as session: branch={}, path={:?}",
                wt.branch, wt.path
            );

            new_sessions.push(session);
            imported += 1;
        }

        if !new_sessions.is_empty() {
            self.store
                .mutate(move |state| {
                    for session in new_sessions {
                        state.add_session(session);
                    }
                })
                .await?;
        }

        if imported > 0 {
            info!(
                "Synced {} unmanaged worktree(s) for project {}",
                imported, project_id
            );
        }

        Ok(imported)
    }

    /// Generate branch name from title
    fn generate_branch_name(&self, title: &str) -> String {
        let sanitized = self.sanitize_name(title);

        let config = self.config_store.read();
        if config.branch_prefix.is_empty() {
            sanitized
        } else {
            format!("{}/{}", config.branch_prefix, sanitized)
        }
    }

    /// Sanitize a name for use as branch/directory name
    fn sanitize_name(&self, name: &str) -> String {
        name.to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppState, Config, ConfigStore, StateStore};
    use tempfile::TempDir;

    fn test_store() -> (TempDir, Arc<StateStore>) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        let store = Arc::new(StateStore::with_path(AppState::new(), path));
        (dir, store)
    }

    fn test_config_store(config: Config) -> (TempDir, Arc<ConfigStore>) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let toml = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&path, toml).unwrap();
        let store = Arc::new(ConfigStore::with_path(config, path));
        (dir, store)
    }

    #[test]
    fn test_sanitize_name() {
        let (_cdir, config_store) = test_config_store(Config::default());
        let (_dir, store) = test_store();
        let manager = SessionManager::new(config_store, store, "");

        assert_eq!(manager.sanitize_name("Hello World"), "hello-world");
        assert_eq!(manager.sanitize_name("Feature/Auth"), "feature-auth");
        assert_eq!(manager.sanitize_name("--test--"), "test");
    }

    #[test]
    fn test_generate_branch_name() {
        let (_cdir, config_store) = test_config_store(Config::default());
        let (_dir, store) = test_store();

        // Without prefix
        let manager = SessionManager::new(config_store, store.clone(), "");
        assert_eq!(manager.generate_branch_name("Feature Auth"), "feature-auth");

        // With prefix
        let config = Config {
            branch_prefix: "cc".to_string(),
            ..Config::default()
        };
        let (_cdir2, config_store2) = test_config_store(config);
        let manager = SessionManager::new(config_store2, store, "");
        assert_eq!(
            manager.generate_branch_name("Feature Auth"),
            "cc/feature-auth"
        );
    }

    #[test]
    fn test_sanitize_name_underscores_preserved() {
        let (_cdir, config_store) = test_config_store(Config::default());
        let (_dir, store) = test_store();
        let manager = SessionManager::new(config_store, store, "");

        assert_eq!(manager.sanitize_name("hello_world"), "hello_world");
    }

    #[test]
    fn test_sanitize_name_consecutive_specials() {
        let (_cdir, config_store) = test_config_store(Config::default());
        let (_dir, store) = test_store();
        let manager = SessionManager::new(config_store, store, "");

        assert_eq!(manager.sanitize_name("a!!b"), "a--b");
    }

    #[test]
    fn test_sanitize_name_all_special() {
        let (_cdir, config_store) = test_config_store(Config::default());
        let (_dir, store) = test_store();
        let manager = SessionManager::new(config_store, store, "");

        assert_eq!(manager.sanitize_name("!!!"), "");
    }

    #[test]
    fn test_sanitize_name_unicode() {
        let (_cdir, config_store) = test_config_store(Config::default());
        let (_dir, store) = test_store();
        let manager = SessionManager::new(config_store, store, "");

        // Unicode alphanumeric chars should be preserved
        let result = manager.sanitize_name("café");
        assert!(result.contains("caf"));
        assert!(result.contains('é'));
    }

    #[test]
    fn test_generate_branch_name_empty_prefix() {
        let (_cdir, config_store) = test_config_store(Config::default());
        let (_dir, store) = test_store();
        let manager = SessionManager::new(config_store, store, "");

        assert_eq!(manager.generate_branch_name("Foo Bar"), "foo-bar");
    }

    #[test]
    fn test_generate_branch_name_slash_in_prefix() {
        let config = Config {
            branch_prefix: "user/cc".to_string(),
            ..Config::default()
        };
        let (_cdir, config_store) = test_config_store(config);
        let (_dir, store) = test_store();
        let manager = SessionManager::new(config_store, store, "");

        assert_eq!(manager.generate_branch_name("Foo"), "user/cc/foo");
    }
}
