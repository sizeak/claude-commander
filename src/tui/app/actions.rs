//! Action handlers: session management, modal submissions, editor/PR integration.

use super::*;

impl App {
    pub(super) fn selected_session_is_creating(&self) -> bool {
        self.ui_state.list_items.iter().any(|item| {
            matches!(
                item,
                SessionListItem::Worktree { id, status, .. }
                if self.ui_state.selected_session_id == Some(*id)
                    && *status == SessionStatus::Creating
            )
        })
    }

    /// Handle selection (attach to session)
    pub(super) async fn handle_select(&mut self) {
        info!(
            "handle_select called, selected_session_id: {:?}",
            self.ui_state.selected_session_id
        );
        if self.selected_session_is_creating() {
            return;
        }
        if let Some(session_id) = self.ui_state.selected_session_id {
            info!("Getting attach command for session: {}", session_id);
            match self.session_manager.get_attach_command(&session_id).await {
                Ok(cmd) => {
                    info!("Got attach command: {}", cmd);
                    // Clear unread flag when attaching
                    let sid = session_id;
                    let _ = self
                        .store
                        .mutate(move |state| {
                            if let Some(session) = state.get_session_mut(&sid) {
                                session.unread = false;
                            }
                        })
                        .await;
                    self.ui_state.attach_command = Some(cmd);
                    self.ui_state.should_quit = true;
                    info!("Set should_quit = true");
                }
                Err(e) => {
                    info!("Failed to get attach command: {}", e);
                    self.ui_state.modal = Modal::Error {
                        message: format!("Cannot attach: {}", e),
                    };
                }
            }
        } else {
            info!("No session selected");
        }
    }

    /// Handle shell selection (attach to shell session)
    pub(super) async fn handle_select_shell(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
        if let Some(session_id) = self.ui_state.selected_session_id {
            match self
                .session_manager
                .get_shell_attach_command(&session_id)
                .await
            {
                Ok(cmd) => {
                    self.ui_state.attach_command = Some(cmd);
                    self.ui_state.should_quit = true;
                }
                Err(e) => {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Cannot open shell: {}", e),
                    };
                }
            }
        } else if let Some(project_id) = self.ui_state.selected_project_id {
            match self
                .session_manager
                .get_project_shell_attach_command(&project_id)
                .await
            {
                Ok(cmd) => {
                    self.ui_state.attach_command = Some(cmd);
                    self.ui_state.should_quit = true;
                }
                Err(e) => {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Cannot open shell: {}", e),
                    };
                }
            }
        }
    }

    /// Resolve the shell toggle pair for a given tmux session name.
    ///
    /// If the current session is a Claude session, returns the shell session name
    /// (creating it if needed). If the current session is already a shell session
    /// (ends with "-sh"), returns the Claude session name.
    pub(super) async fn resolve_shell_toggle_pair(
        &mut self,
        current_tmux_name: &str,
    ) -> crate::error::Result<String> {
        if current_tmux_name.ends_with("-sh") {
            // We're in a shell session — the Claude session is the name without "-sh"
            let claude_name = current_tmux_name.trim_end_matches("-sh").to_string();
            // Verify the Claude session exists
            if self
                .session_manager
                .tmux
                .session_exists(&claude_name)
                .await?
            {
                return Ok(claude_name);
            }
            return Err(crate::error::Error::Session(
                crate::error::SessionError::TmuxSessionNotFound(claude_name),
            ));
        }

        // We're in a Claude session — find the matching session ID and ensure shell exists
        let session_id = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .find(|s| s.tmux_session_name == current_tmux_name)
                .map(|s| s.id)
        };

        if let Some(session_id) = session_id {
            let shell_name = self
                .session_manager
                .ensure_shell_session(&session_id)
                .await?;
            return Ok(shell_name);
        }

        // Try project-level shell
        let project_id = {
            let state = self.store.read().await;
            state
                .projects
                .values()
                .find(|p| p.shell_tmux_session_name.as_deref() == Some(current_tmux_name))
                .map(|p| p.id)
        };

        if let Some(project_id) = project_id {
            let shell_name = self
                .session_manager
                .ensure_project_shell_session(&project_id)
                .await?;
            return Ok(shell_name);
        }

        Err(crate::error::Error::Session(
            crate::error::SessionError::TmuxSessionNotFound(format!(
                "No session found for tmux name: {}",
                current_tmux_name
            )),
        ))
    }

    /// Open the editor for the worktree associated with a given tmux session
    /// name. Used when the user presses Ctrl+. while attached to a tmux
    /// session — the tmux session itself is not affected, we simply launch
    /// the configured editor pointing at the session's worktree. This runs
    /// while we are *between* attaches, so the TUI is torn down and raw mode
    /// is already disabled.
    pub(super) async fn open_editor_for_tmux_session(&mut self, tmux_session_name: &str) {
        // Shell sessions are named `<claude_name>-sh`; the worktree is owned
        // by the underlying Claude session.
        let lookup_name = tmux_session_name
            .strip_suffix("-sh")
            .unwrap_or(tmux_session_name)
            .to_string();

        let path = {
            let state = self.store.read().await;
            state
                .sessions
                .values()
                .find(|s| s.tmux_session_name == lookup_name)
                .map(|s| s.worktree_path.clone())
        };

        let Some(path) = path else {
            warn!(
                "OpenEditor: no session found for tmux name '{}'",
                tmux_session_name
            );
            return;
        };

        let Some(editor) = self.config.resolve_editor() else {
            warn!("OpenEditor: no editor configured");
            return;
        };

        if self.config.is_gui_editor(&editor) {
            // GUI editor: spawn detached and return — tmux session is
            // untouched and we'll re-attach immediately.
            info!("OpenEditor: launching GUI editor '{}' at {}", editor, path.display());
            if let Err(e) = std::process::Command::new(&editor).arg(&path).spawn() {
                warn!("Failed to launch GUI editor '{}': {}", editor, e);
            }
        } else {
            // Terminal editor: run foreground, inheriting stdio. Raw mode is
            // already off (attach_to_session disabled it on exit) so the
            // editor gets a cooked terminal. When it returns we loop back
            // into attach_to_session with the same tmux session name.
            info!(
                "OpenEditor: launching terminal editor '{}' at {}",
                editor,
                path.display()
            );
            if let Err(e) = std::process::Command::new(&editor).arg(&path).status() {
                warn!("Failed to launch terminal editor '{}': {}", editor, e);
            }
        }
    }

    /// Handle open in editor command
    pub(super) async fn handle_open_in_editor(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
        let path = {
            let state = self.store.read().await;
            if let Some(session_id) = self.ui_state.selected_session_id {
                state
                    .sessions
                    .get(&session_id)
                    .map(|s| s.worktree_path.clone())
            } else if let Some(project_id) = self.ui_state.selected_project_id {
                state.projects.get(&project_id).map(|p| p.repo_path.clone())
            } else {
                None
            }
        };

        let Some(path) = path else {
            return;
        };

        let Some(editor) = self.config.resolve_editor() else {
            self.ui_state.modal = Modal::Error {
                message: "No editor configured. Set 'editor' in config.toml or \
                          set $VISUAL / $EDITOR."
                    .to_string(),
            };
            return;
        };

        if self.config.is_gui_editor(&editor) {
            // GUI editor: spawn detached, TUI stays up
            if let Err(e) = std::process::Command::new(&editor).arg(&path).spawn() {
                self.ui_state.modal = Modal::Error {
                    message: format!("Failed to launch '{}': {}", editor, e),
                };
            }
        } else {
            // Terminal editor: tear down TUI, run foreground, restore
            self.ui_state.editor_command = Some((editor, path));
            self.ui_state.should_quit = true;
        }
    }

    /// Handle "open PR in browser" — looks up the selected session's
    /// `pr_url` and launches the OS default handler (`open` on macOS,
    /// `xdg-open` on Linux, `cmd /c start` on Windows).
    pub(super) async fn handle_open_pull_request(&mut self) {
        let Some(session_id) = self.ui_state.selected_session_id else {
            return;
        };
        let pr_url = {
            let state = self.store.read().await;
            state
                .sessions
                .get(&session_id)
                .and_then(|s| s.pr_url.clone())
        };
        let Some(url) = pr_url else {
            self.ui_state.status_message = Some((
                "No PR associated with this session".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };

        let result = if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(&url).spawn()
        } else if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/c", "start", "", &url])
                .spawn()
        } else {
            std::process::Command::new("xdg-open").arg(&url).spawn()
        };

        if let Err(e) = result {
            self.ui_state.modal = Modal::Error {
                message: format!("Failed to open PR in browser: {}", e),
            };
        }
    }

    /// Handle new session command
    pub(super) fn handle_new_session(&mut self) {
        if let Some(project_id) = self.ui_state.selected_project_id {
            self.ui_state.modal = Modal::Input {
                title: "New Session".to_string(),
                prompt: "Enter session name:".to_string(),
                value: String::new(),
                on_submit: InputAction::CreateSession { project_id },
            };
        } else {
            self.ui_state.status_message = Some((
                "Select a project first (use N to add one)".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
        }
    }

    /// Open the quick-switch modal with all sessions
    pub(super) async fn open_quick_switch(&mut self) {
        let matches = self.gather_quick_switch_matches("").await;
        self.ui_state.modal = Modal::QuickSwitch {
            query: String::new(),
            matches,
            selected_idx: 0,
        };
    }

    /// Gather session matches for a query (empty query = all sessions)
    pub(super) async fn gather_quick_switch_matches(&self, query: &str) -> Vec<QuickSwitchMatch> {
        let state = self.store.read().await;
        let mut matches = Vec::new();

        for session in state.sessions.values() {
            if session.status == SessionStatus::Creating {
                continue;
            }
            if !query.is_empty() && !session.matches_query(query) {
                continue;
            }
            let project_name = state
                .get_project(&session.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            matches.push(QuickSwitchMatch {
                session_id: session.id,
                title: session.title.clone(),
                branch: session.branch.clone(),
                project_name,
                status: session.status,
            });
        }

        // Sort by title for predictable ordering
        matches.sort_by(|a, b| a.title.cmp(&b.title));
        matches
    }

    /// Re-filter the quick-switch matches based on the current query.
    /// Rebuilds from list_items so backspace can widen results.
    pub(super) fn refilter_quick_switch(&mut self) {
        if let Modal::QuickSwitch {
            query,
            matches,
            selected_idx,
        } = &mut self.ui_state.modal
        {
            let query_lower = query.to_lowercase();
            // Build project name lookup from list items
            let mut project_names: std::collections::HashMap<SessionId, String> =
                std::collections::HashMap::new();
            let mut current_project_name = String::new();
            for item in &self.ui_state.list_items {
                match item {
                    SessionListItem::Project { name, .. } => {
                        current_project_name = name.clone();
                    }
                    SessionListItem::Worktree { id, .. } => {
                        project_names.insert(*id, current_project_name.clone());
                    }
                }
            }

            *matches = self
                .ui_state
                .list_items
                .iter()
                .filter_map(|item| {
                    if let SessionListItem::Worktree {
                        id,
                        title,
                        branch,
                        status,
                        ..
                    } = item
                    {
                        let project_name = project_names.get(id).cloned().unwrap_or_default();
                        if query_lower.is_empty() || title.to_lowercase().contains(&query_lower) {
                            return Some(QuickSwitchMatch {
                                session_id: *id,
                                title: title.clone(),
                                branch: branch.clone(),
                                project_name,
                                status: *status,
                            });
                        }
                    }
                    None
                })
                .collect();

            // Clamp selection
            if *selected_idx >= matches.len() {
                *selected_idx = matches.len().saturating_sub(1);
            }
        }
    }

    /// Handle remove project - show confirmation (only when a project row is selected)
    pub(super) fn handle_remove_project(&mut self) {
        if self.ui_state.selected_session_id.is_none()
            && let Some(project_id) = self.ui_state.selected_project_id
        {
            self.ui_state.modal = Modal::Confirm {
                    title: "Remove Project".to_string(),
                    message: "Are you sure you want to remove this project?\nThis will kill all sessions and remove all worktrees.".to_string(),
                    on_confirm: ConfirmAction::RemoveProject { project_id },
                };
        }
    }

    /// Handle restart session - show confirmation
    pub(super) fn handle_restart_session(&mut self) {
        if let Some(session_id) = self.ui_state.selected_session_id {
            let message = if self.config.resume_session {
                "This will kill the current tmux session and start a fresh one.\nClaude will pick up where it left off via /resume.".to_string()
            } else {
                "This will kill the current tmux session and start a fresh one.\nIf you want to pick up where you left off, you can use /resume.".to_string()
            };
            self.ui_state.modal = Modal::Confirm {
                title: "Restart Session".to_string(),
                message,
                on_confirm: ConfirmAction::RestartSession { session_id },
            };
        }
    }

    /// Handle delete session - show confirmation
    pub(super) fn handle_delete_session(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
        if let Some(session_id) = self.ui_state.selected_session_id {
            self.ui_state.modal = Modal::Confirm {
                title: "Delete Session".to_string(),
                message: "Are you sure you want to delete this session?\nThis will kill the tmux session and remove the worktree.".to_string(),
                on_confirm: ConfirmAction::DeleteSession { session_id },
            };
        }
    }

    /// Handle input modal submission
    pub(super) async fn handle_input_submit(&mut self, action: InputAction, value: String) {
        match action {
            InputAction::CreateSession { project_id } => {
                if value.trim().is_empty() {
                    self.ui_state.status_message = Some((
                        "Session name cannot be empty".to_string(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                    return;
                }

                // Insert placeholder session immediately (no blocking modal)
                self.ui_state.modal = Modal::None;
                let session_id = match self
                    .session_manager
                    .prepare_session(&project_id, value, None)
                    .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to create session: {}", e),
                        };
                        return;
                    }
                };

                // Refresh list and select the new placeholder
                self.refresh_list_items().await;
                if let Some(idx) = self.ui_state.list_items.iter().position(|item| {
                    matches!(item, SessionListItem::Worktree { id, .. } if *id == session_id)
                }) {
                    self.ui_state.list_state.select(Some(idx));
                }
                self.update_selection();

                // Spawn background task for heavy work
                let session_manager = self.session_manager.clone();
                let tx = self.event_loop.sender();
                tokio::spawn(async move {
                    match session_manager.finalize_session(&session_id).await {
                        Ok(sid) => {
                            let _ = tx
                                .send(AppEvent::StateUpdate(StateUpdate::SessionCreated {
                                    session_id: sid,
                                }))
                                .await;
                        }
                        Err(e) => {
                            let _ = tx
                                .send(AppEvent::StateUpdate(StateUpdate::SessionCreateFailed {
                                    session_id,
                                    message: format!("Failed to create session: {}", e),
                                }))
                                .await;
                        }
                    }
                });
            }
            InputAction::AddProject => {
                let expanded = crate::tui::path_completer::expand_tilde(value.trim());
                let path = PathBuf::from(expanded);
                if !path.exists() {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Path does not exist: {}", path.display()),
                    };
                    return;
                }

                match self.session_manager.add_project(path).await {
                    Ok(project_id) => {
                        self.ui_state.status_message = Some((
                            format!("Added project {}", project_id),
                            Instant::now() + Duration::from_secs(3),
                        ));
                        self.refresh_list_items().await;
                        // Select the newly added project
                        if let Some(idx) = self.ui_state.list_items.iter().position(|item| {
                            matches!(item, SessionListItem::Project { id, .. } if *id == project_id)
                        }) {
                            self.ui_state.list_state.select(Some(idx));
                        }
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to add project: {}", e),
                        };
                    }
                }
            }
        }
    }

    /// Handle confirmation
    pub(super) async fn handle_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteSession { session_id } => {
                // 1. Capture session data before removal
                let cleanup_data = {
                    let state = self.store.read().await;
                    state.get_session(&session_id).map(|s| {
                        let repo_path = state
                            .get_project(&s.project_id)
                            .map(|p| p.repo_path.clone());
                        (
                            s.tmux_session_name.clone(),
                            s.shell_tmux_session_name.clone(),
                            s.worktree_path.clone(),
                            repo_path,
                        )
                    })
                };

                // 2. Remove from state immediately so the UI updates
                if let Err(e) = self
                    .store
                    .mutate(move |state| {
                        state.remove_session(&session_id);
                    })
                    .await
                {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Failed to save state: {}", e),
                    };
                    return;
                }
                self.ui_state.selected_session_id = None;
                self.ui_state.status_message = Some((
                    "Session deleted".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
                self.refresh_list_items().await;

                // 3. Spawn background cleanup (kill tmux + remove worktree)
                if let Some((tmux_name, shell_tmux_name, worktree_path, repo_path)) = cleanup_data {
                    let tmux = self.session_manager.tmux.clone();
                    let tx = self.event_loop.sender();
                    tokio::spawn(async move {
                        background::cleanup_session_tmux(
                            &tmux,
                            &tmux_name,
                            shell_tmux_name.as_deref(),
                            repo_path
                                .as_ref()
                                .map(|rp| (worktree_path.as_path(), rp.as_path())),
                            &tx,
                        )
                        .await;
                    });
                }
            }
            ConfirmAction::RestartSession { session_id } => {
                match self.session_manager.restart_session(&session_id).await {
                    Ok(_) => {
                        self.ui_state.status_message = Some((
                            "Session restarted".to_string(),
                            Instant::now() + Duration::from_secs(3),
                        ));
                        self.refresh_list_items().await;
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to restart: {}", e),
                        };
                    }
                }
            }
            ConfirmAction::RemoveProject { project_id } => {
                // 1. Capture project and session data before removal
                let cleanup_data = {
                    let state = self.store.read().await;
                    state.get_project(&project_id).map(|project| {
                        let repo_path = project.repo_path.clone();
                        let shell_tmux = project.shell_tmux_session_name.clone();
                        let sessions: Vec<_> = project
                            .worktrees
                            .iter()
                            .filter_map(|sid| {
                                state.get_session(sid).map(|s| {
                                    (
                                        s.tmux_session_name.clone(),
                                        s.shell_tmux_session_name.clone(),
                                        s.worktree_path.clone(),
                                    )
                                })
                            })
                            .collect();
                        (repo_path, shell_tmux, sessions)
                    })
                };

                // 2. Remove from state immediately so the UI updates
                if let Err(e) = self
                    .store
                    .mutate(move |state| {
                        state.remove_project(&project_id);
                    })
                    .await
                {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Failed to save state: {}", e),
                    };
                    return;
                }
                self.ui_state.selected_project_id = None;
                self.ui_state.status_message = Some((
                    "Project removed".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
                self.refresh_list_items().await;

                // 3. Spawn background cleanup (kill all tmux sessions + remove worktrees)
                if let Some((repo_path, shell_tmux, sessions)) = cleanup_data {
                    let tmux = self.session_manager.tmux.clone();
                    let tx = self.event_loop.sender();
                    tokio::spawn(async move {
                        // Kill project shell tmux session
                        if let Some(ref shell_name) = shell_tmux {
                            let _ = tmux.kill_session(shell_name).await;
                        }
                        // Kill all session tmux sessions + remove worktrees
                        for (tmux_name, shell_tmux_name, worktree_path) in &sessions {
                            background::cleanup_session_tmux(
                                &tmux,
                                tmux_name,
                                shell_tmux_name.as_deref(),
                                Some((worktree_path.as_path(), repo_path.as_path())),
                                &tx,
                            )
                            .await;
                        }
                    });
                }
            }
        }
    }
}
