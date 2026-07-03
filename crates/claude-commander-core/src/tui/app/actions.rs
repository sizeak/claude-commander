//! User actions: session selection, creation, deletion, editor/PR/shell interactions.

use super::*;

/// Which cascade entrypoint `run_cascade_action` should invoke.
#[derive(Debug, Clone, Copy)]
enum CascadeAction {
    Start,
    Resume,
}

/// Maximum number of rows rendered in a scrollable list modal at once.
///
/// Shared between the render layer and the input handler so the scroll
/// offset and the visible window agree. Used by both the quick-switch
/// palette and the path-input completions list.
pub(super) const LIST_MAX_VISIBLE: usize = 10;

/// Return the `scroll` offset that keeps `selected_idx` inside a visible
/// window of `visible_rows` rows, starting from the caller's current
/// scroll position. Handles all four cases (above window, below window,
/// wrap-around onto either end, and no-op when already in view) in a single
/// pure function so it can be unit-tested independently.
pub(super) fn adjust_list_scroll(selected_idx: usize, scroll: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        return 0;
    }
    if selected_idx < scroll {
        selected_idx
    } else if selected_idx >= scroll + visible_rows {
        selected_idx + 1 - visible_rows
    } else {
        scroll
    }
}

/// Confirmation prompt for deleting a session. Names the session by its
/// title when known so the user can tell what they're about to destroy;
/// falls back to a generic phrasing if the title can't be resolved.
pub(super) fn delete_confirm_message(
    title: Option<&str>,
    retarget: Option<(usize, &str)>,
) -> String {
    let subject = match title {
        Some(title) => format!("\"{title}\""),
        None => "this session".to_string(),
    };
    let mut message = format!(
        "Are you sure you want to delete {subject}?\nThis will kill the tmux session and remove the worktree."
    );
    if let Some((count, new_base)) = retarget {
        let plural = if count == 1 { "session" } else { "sessions" };
        message.push_str(&format!(
            "\n{count} stacked {plural} will be retargeted onto \"{new_base}\"."
        ));
    }
    message
}

/// One mouse-wheel step over a list selection: move a single row, clamping
/// at the ends rather than wrapping like keyboard navigation — a wheel tick
/// at the bottom of a list jumping back to the top would be disorienting.
pub(super) fn wheel_step(selected_idx: usize, down: bool, len: usize) -> usize {
    if down {
        (selected_idx + 1).min(len.saturating_sub(1))
    } else {
        selected_idx.saturating_sub(1)
    }
}

impl App {
    /// Open `Modal::PathInput` at the current working directory with its
    /// subdirectory list already populated.
    ///
    /// The initial value is `cwd/` (trailing slash appended) so
    /// `list_matching_dirs` returns the children of cwd rather than its
    /// siblings — which is what users almost always want for Add Project /
    /// Scan Directory.
    pub(super) fn open_path_input(
        &mut self,
        title: String,
        prompt: String,
        on_submit: InputAction,
    ) {
        let mut value = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if !value.ends_with('/') {
            value.push('/');
        }
        let mut completer = PathCompleter::new();
        completer.refilter(&value);
        self.ui_state.modal = Modal::PathInput {
            title,
            prompt,
            value: value.into(),
            on_submit,
            completer,
            scroll: 0,
        };
    }

    /// Check if the selected session is in Creating state
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
        let Some(session_id) = self.ui_state.selected_session_id else {
            info!("No session selected");
            return;
        };
        // Validate against the cached snapshot so a non-attachable session
        // reports immediately; the backend revives a dead-but-attachable tmux
        // session when the attach actually runs.
        match self
            .session(SessionRef::local(session_id))
            .map(|s| s.status)
        {
            Some(status) if status.can_attach() => {
                // Clear unread when attaching.
                let _ = self.local_arc().mark_read(session_id).await;
                self.ui_state.attach_request = Some(AttachTarget::Session {
                    session: SessionRef::local(session_id),
                    kind: AttachKind::Agent,
                });
                self.ui_state.should_quit = true;
            }
            Some(_) => {
                self.ui_state.modal = Modal::Error {
                    message: "Cannot attach: session is not running".to_string(),
                };
            }
            None => {}
        }
    }

    /// Handle shell selection (attach to shell session)
    pub(super) async fn handle_select_shell(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
        if let Some(session_id) = self.ui_state.selected_session_id {
            // The backend creates the `-sh` pair on demand; a failure surfaces
            // as an error modal once the attach runs.
            self.ui_state.attach_request = Some(AttachTarget::Session {
                session: SessionRef::local(session_id),
                kind: AttachKind::Shell,
            });
            self.ui_state.should_quit = true;
        } else if let Some(project_id) = self.ui_state.selected_project_id {
            // Project shells have no `SessionId` — resolve the name locally.
            let Some(be) = self.local_backend() else {
                return;
            };
            match be.project_shell_name(project_id).await {
                Ok(name) => {
                    self.ui_state.attach_request = Some(AttachTarget::LocalName(name));
                    self.ui_state.should_quit = true;
                }
                Err(e) => {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Cannot open shell: {e}"),
                    };
                }
            }
        }
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

        let path = self
            .local_view()
            .snapshot
            .sessions
            .iter()
            .find(|s| s.tmux_session_name == lookup_name)
            .map(|s| s.worktree_path.clone());

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
            info!(
                "OpenEditor: launching GUI editor '{}' at {}",
                editor,
                path.display()
            );
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
        let path = if let Some(session_id) = self.ui_state.selected_session_id {
            self.session(SessionRef::local(session_id))
                .map(|s| s.worktree_path.clone())
        } else if let Some(project_id) = self.ui_state.selected_project_id {
            self.local_view()
                .snapshot
                .projects
                .iter()
                .find(|p| p.id == project_id)
                .map(|p| p.repo_path.clone())
        } else {
            None
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
        let pr_url = self
            .session(SessionRef::local(session_id))
            .and_then(|s| s.pr_url.clone());
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

    /// Open (creating or reviving if needed) the persistent commander session,
    /// then hand off to the attach loop the same way `handle_select` does.
    pub(super) async fn handle_open_commander(&mut self) {
        // Primary gate is restart-required: it keys off the init snapshot so it
        // stays consistent with the chip/poller, which are wired at init. A
        // runtime toggle therefore can't half-enable the commander (attachable
        // but with no live chip).
        if !self.commander_enabled_at_init {
            self.ui_state.status_message = Some((
                "Commander session is disabled — enable it in settings, then restart".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        }

        // The commander is a local-only session (no `SessionId`); ensure it via
        // the local backend, which reuses the shared tmux executor and re-checks
        // the live flag (short-circuiting with `CommanderDisabled` — a backstop
        // for the toggle-off-while-running edge above the snapshot).
        let Some(be) = self.local_backend() else {
            return;
        };
        let cmd = crate::cli_args::cli_command();
        let result = be.ensure_commander(&self.config, &cmd).await;

        match result {
            Ok(name) => {
                self.ui_state.attach_request = Some(AttachTarget::LocalName(name));
                self.ui_state.should_quit = true;
            }
            Err(crate::Error::Session(crate::error::SessionError::CommanderDisabled)) => {
                self.ui_state.status_message = Some((
                    "Commander session is disabled — enable it in settings".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
            Err(e) => {
                self.ui_state.modal = Modal::Error {
                    message: format!("Failed to open commander: {}", e),
                };
            }
        }
    }

    /// Handle new session command
    /// Build the program picker for a new-session dialog: the configured
    /// choices with the default program pre-selected, name field focused first.
    pub(super) fn new_program_picker(&self) -> super::ProgramPicker {
        super::ProgramPicker {
            choices: self.config.program_choices(),
            selected: self.config.default_program_index(),
        }
    }

    /// Build the project picker for a new-session dialog: every project sorted
    /// by name, with `default` pre-selected.
    async fn new_project_picker(&self, default: ProjectId) -> super::ProjectPicker {
        let mut choices: Vec<super::ProjectChoice> = self
            .local_view()
            .snapshot
            .projects
            .iter()
            .map(|p| super::ProjectChoice {
                id: p.id,
                name: p.name.clone(),
                repo_path: p.repo_path.clone(),
            })
            .collect();
        choices.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        super::ProjectPicker::new(choices, default)
    }

    pub(super) async fn handle_new_session(&mut self) {
        if let Some(project_id) = self.ui_state.selected_project_id {
            let repo_path = self.project(project_id).map(|p| p.repo_path.clone());
            let existing_branches = repo_path.and_then(|p| existing_branch_names(&p));
            // Capture the section under the cursor now, so a background list
            // refresh while the modal is open can't change where the new
            // session lands.
            let section = self
                .ui_state
                .list_state
                .selected()
                .and_then(|idx| super::selection::section_at(&self.ui_state.list_items, idx));
            let project_picker = self.new_project_picker(project_id).await;
            self.ui_state.modal = Modal::Input {
                title: "New Session".to_string(),
                prompt: "Enter session name:".to_string(),
                value: super::Input::default(),
                on_submit: InputAction::CreateSession {
                    project_id,
                    section,
                },
                existing_branches,
                project_picker: Some(project_picker),
                program_picker: Some(self.new_program_picker()),
                focus: super::InputFocus::Name,
                expanded: false,
            };
        } else {
            self.ui_state.status_message = Some((
                "Select a project first (use N to add one)".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
        }
    }

    /// Handle "new stacked session" — create a session on top of the stack the
    /// selected session belongs to. Starting from the selected session, we
    /// walk to the top of its stack (the leaf, if any), so pressing the
    /// hotkey from any row in the stack always produces a sibling stacked on
    /// the current topmost member. Selecting a standalone session starts a
    /// new stack rooted there.
    pub(super) async fn handle_new_stacked_session(&mut self) {
        let Some(selected_session_id) = self.ui_state.selected_session_id else {
            self.ui_state.status_message = Some((
                "Select a session to stack on top of".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };
        let resolved = {
            let snap = &self.local_view().snapshot;
            snap.sessions
                .iter()
                .find(|s| s.session_id == selected_session_id)
                .and_then(|selected| {
                    let project_id = selected.project_id;
                    let project = snap.projects.iter().find(|p| p.id == project_id)?;
                    let project_sessions: Vec<&crate::api::SessionInfo> = project
                        .session_ids
                        .iter()
                        .filter_map(|sid| snap.sessions.iter().find(|s| s.session_id == *sid))
                        .collect();
                    let top_id = crate::session::stack_top(selected_session_id, &project_sessions);
                    let top = snap.sessions.iter().find(|s| s.session_id == top_id)?;
                    Some((
                        project_id,
                        top.session_id,
                        top.branch.clone(),
                        top.title.clone(),
                    ))
                })
        };
        let Some((project_id, parent_session_id, parent_branch, parent_title)) = resolved else {
            return;
        };
        let repo_path = self.project(project_id).map(|p| p.repo_path.clone());
        let existing_branches = repo_path.and_then(|p| existing_branch_names(&p));
        self.ui_state.modal = Modal::Input {
            title: format!("New Session Stacked on \"{}\"", parent_title),
            prompt: "Enter session name:".to_string(),
            value: super::Input::default(),
            on_submit: InputAction::CreateStackedSession {
                project_id,
                parent_session_id,
                parent_branch,
            },
            existing_branches,
            project_picker: None,
            program_picker: Some(self.new_program_picker()),
            focus: super::InputFocus::Name,
            expanded: false,
        };
    }

    /// Handle `Cascade merge main` — walk to the base of the selected
    /// session's stack and merge main → base → each descendant. Pauses on
    /// the first conflict; surface the outcome as a status-message toast.
    pub(super) async fn handle_cascade_merge_main(&mut self) {
        let Some(selected_session_id) = self.ui_state.selected_session_id else {
            self.ui_state.status_message = Some((
                "Select a session in a stack to cascade from".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };
        self.run_cascade_action(selected_session_id, CascadeAction::Start);
    }

    /// Handle `Cascade resume` — continue a previously paused cascade.
    pub(super) async fn handle_cascade_resume(&mut self) {
        let paused_at = self.local_view().snapshot.cascade_paused;
        let Some(sid) = paused_at else {
            self.ui_state.status_message = Some((
                "No cascade in progress".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };
        self.run_cascade_action(sid, CascadeAction::Resume);
    }

    /// Handle `Push stack` — push every branch in the selected session's
    /// stack to origin, in base→leaf order, on a background task.
    pub(super) fn handle_push_stack(&mut self) {
        let Some(session_id) = self.ui_state.selected_session_id else {
            self.ui_state.status_message = Some((
                "Select a session in a stack to push".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };

        // Close the modal (if dispatched from the palette) and drop a toast
        // so the TUI renders immediately before the push spawns.
        self.ui_state.modal = Modal::None;
        self.ui_state.status_message = Some((
            "Push stack starting…".to_string(),
            Instant::now() + Duration::from_secs(30),
        ));

        // The backend detects agent states itself, so the TUI no longer passes
        // its cached map. Records the outcome in the operation ledger.
        let backend = self.local_arc();
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let result = backend
                .push_stack(session_id)
                .await
                .map_err(|e| e.to_string());
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::PushStackFinished {
                    result,
                }))
                .await;
        });
    }

    pub(super) async fn handle_push_stack_finished(
        &mut self,
        result: std::result::Result<crate::api::OperationStatus, String>,
    ) {
        self.refresh_local_view().await;
        self.refresh_list_items().await;
        let (msg, secs) = match result {
            Ok(status) => match status.outcome {
                crate::api::OperationOutcome::Succeeded { detail } => {
                    (format!("Push stack complete: {detail}"), 5)
                }
                crate::api::OperationOutcome::Paused { detail } => {
                    (format!("Push stack paused: {detail}"), 15)
                }
                crate::api::OperationOutcome::Failed { error } => {
                    (format!("Push stack failed: {error}"), 15)
                }
            },
            Err(e) => (format!("Push stack failed: {e}"), 15),
        };
        self.ui_state.status_message = Some((msg, Instant::now() + Duration::from_secs(secs)));
    }

    /// Handle `Cascade abandon` — clear the paused state without merging.
    pub(super) async fn handle_cascade_abandon(&mut self) {
        match self.local_arc().cascade_abandon().await {
            Ok(()) => {
                self.ui_state.status_message = Some((
                    "Cascade pause cleared".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
                self.refresh_local_view().await;
                self.refresh_list_items().await;
            }
            Err(e) => {
                self.ui_state.status_message = Some((
                    format!("Cascade abandon failed: {e}"),
                    Instant::now() + Duration::from_secs(5),
                ));
            }
        }
    }

    fn run_cascade_action(&mut self, session_id: SessionId, action: CascadeAction) {
        // Close any open modal (e.g. the palette that dispatched us) and
        // drop a "running" toast immediately so the TUI redraws with neither
        // blocked before the cascade starts. The cascade itself runs on a
        // background task so git merges / fetches don't stall the event loop.
        self.ui_state.modal = Modal::None;
        let action_label = match action {
            CascadeAction::Start => "Cascade merge starting…",
            CascadeAction::Resume => "Resuming cascade merge…",
        };
        self.ui_state.status_message = Some((
            action_label.to_string(),
            Instant::now() + Duration::from_secs(30),
        ));

        // The backend detects agent states itself and records the outcome in
        // the operation ledger, returning the recorded status.
        let backend = self.local_arc();
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let result = match action {
                CascadeAction::Start => backend.cascade_merge(session_id).await,
                CascadeAction::Resume => backend.cascade_resume().await,
            };
            let result = result.map_err(|e| e.to_string());
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::CascadeFinished {
                    result,
                }))
                .await;
        });
    }

    pub(super) async fn handle_cascade_finished(
        &mut self,
        result: std::result::Result<crate::api::OperationStatus, String>,
    ) {
        self.refresh_local_view().await;
        self.refresh_list_items().await;
        let (msg, secs) = match result {
            Ok(status) => match status.outcome {
                crate::api::OperationOutcome::Succeeded { detail } => {
                    (format!("Cascade complete: {detail}"), 5)
                }
                crate::api::OperationOutcome::Paused { detail } => (
                    format!("Cascade {detail}. Resolve conflicts and run `Cascade resume`."),
                    15,
                ),
                crate::api::OperationOutcome::Failed { error } => {
                    (format!("Cascade failed: {error}"), 10)
                }
            },
            Err(e) => (format!("Cascade failed: {e}"), 10),
        };
        self.ui_state.status_message = Some((msg, Instant::now() + Duration::from_secs(secs)));
    }

    /// Open the Checkout Branch modal.
    ///
    /// Loads the current list of branches synchronously via gix and kicks
    /// off `git fetch origin` in a background task so the list can be
    /// refreshed once remote changes are pulled in.
    pub(super) async fn handle_checkout_branch(&mut self) {
        let Some(project_id) = self.ui_state.selected_project_id else {
            self.ui_state.status_message = Some((
                "Select a project first (use N to add one)".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };

        let repo_path = match self.project(project_id) {
            Some(p) => p.repo_path.clone(),
            None => {
                self.ui_state.modal = Modal::Error {
                    message: "Project not found".to_string(),
                };
                return;
            }
        };

        let all_branches = match load_branch_entries(&repo_path) {
            Ok(b) => b,
            Err(e) => {
                self.ui_state.modal = Modal::Error {
                    message: format!("Failed to list branches: {}", e),
                };
                return;
            }
        };

        let filtered = all_branches.clone();
        self.ui_state.modal = Modal::CheckoutBranch {
            project_id,
            query: super::Input::default(),
            all_branches,
            filtered,
            selected_idx: 0,
            scroll: 0,
            fetching: true,
        };

        // Spawn `git fetch origin` in the background; when it finishes,
        // post a CheckoutFetchComplete state update so the modal (if still
        // open) can refresh its list.
        let tx = self.event_loop.sender();
        let repo_path_bg = repo_path.clone();
        tokio::spawn(async move {
            let _ = tokio::process::Command::new("git")
                .current_dir(&repo_path_bg)
                .args(["fetch", "origin"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await;

            // Re-list branches after the fetch. Run the sync gix call in a
            // blocking task so we don't stall the async runtime.
            let branches = tokio::task::spawn_blocking(move || {
                crate::git::GitBackend::open(&repo_path_bg)
                    .and_then(|b| b.list_branches())
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();

            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::CheckoutFetchComplete {
                    project_id,
                    branches,
                }))
                .await;
        });
    }

    /// Case-insensitive filter over the Checkout modal's branch list.
    pub(super) fn refilter_checkout_branches(&mut self) {
        if let Modal::CheckoutBranch {
            query,
            all_branches,
            filtered,
            selected_idx,
            scroll,
            ..
        } = &mut self.ui_state.modal
        {
            let q = query.value().to_lowercase();
            *filtered = if q.is_empty() {
                all_branches.clone()
            } else {
                all_branches
                    .iter()
                    .filter(|b| {
                        b.local_name.to_lowercase().contains(&q)
                            || b.display_name.to_lowercase().contains(&q)
                    })
                    .cloned()
                    .collect()
            };

            if *selected_idx >= filtered.len() {
                *selected_idx = filtered.len().saturating_sub(1);
            }
            if *scroll > *selected_idx {
                *scroll = *selected_idx;
            }
        }
    }

    /// Start creating a worktree session from an existing branch.
    ///
    /// `branch_name` is the local branch name (remote tracking refs should
    /// already have had their `origin/` prefix stripped before calling).
    /// The session title is derived from the branch name so the worktree
    /// directory uses the same naming as a manually-named new session.
    pub(super) async fn start_checkout_session(
        &mut self,
        project_id: ProjectId,
        branch_name: String,
    ) {
        let branch_name = branch_name.trim().to_string();
        if branch_name.is_empty() {
            return;
        }

        let Some(project_path) = self.project(project_id).map(|p| p.repo_path.clone()) else {
            self.ui_state.modal = Modal::Error {
                message: "Project not found".to_string(),
            };
            return;
        };

        // Use the branch name verbatim as the session title. This keeps
        // `display_branch` from rendering a redundant `[branch]` annotation
        // in the list (it short-circuits on exact title == branch match)
        // and the worktree directory still comes out sensibly because
        // `sanitize_name` handles slashes and special chars. `base_branch`
        // forks the worktree from the existing branch.
        self.spawn_create_session(crate::api::CreateSessionOpts {
            project_path,
            title: branch_name.clone(),
            program: None,
            initial_prompt: None,
            effort: None,
            mode: None,
            base_branch: Some(branch_name),
            section: None,
            stack_parent: None,
        });
    }

    /// Open the quick-switch palette in the given mode.
    pub(super) async fn open_quick_switch_with_mode(&mut self, mode: PaletteMode) {
        let matches = self.build_palette_items(mode, "").await;
        self.ui_state.modal = Modal::QuickSwitch {
            mode,
            query: super::Input::default(),
            matches,
            selected_idx: 0,
            scroll: 0,
        };
    }

    /// Gather session matches for a query (empty query = all sessions).
    ///
    /// Non-empty queries are ranked by fuzzy score (best match first);
    /// empty queries fall back to alphabetical title order.
    pub(super) async fn gather_quick_switch_matches(&self, query: &str) -> Vec<QuickSwitchMatch> {
        let mut scored: Vec<(i64, QuickSwitchMatch)> = Vec::new();

        for session in &self.local_view().snapshot.sessions {
            if session.status == SessionStatus::Creating {
                continue;
            }
            // Best fuzzy score across title/branch/program — mirrors
            // `WorktreeSession::fuzzy_score` over the DTO fields.
            let Some(score) = [
                session.title.as_str(),
                session.branch.as_str(),
                session.program.as_str(),
            ]
            .iter()
            .filter_map(|s| crate::fuzzy::fuzzy_score(s, query))
            .max() else {
                continue;
            };
            scored.push((
                score,
                QuickSwitchMatch {
                    session_id: session.session_id,
                    title: session.title.clone(),
                    branch: session.branch.clone(),
                    project_name: session.project_name.clone(),
                    status: session.status,
                },
            ));
        }

        if query.is_empty() {
            scored.sort_by(|a, b| a.1.title.cmp(&b.1.title));
        } else {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
        }
        scored.into_iter().map(|(_, m)| m).collect()
    }

    /// Compute the *effective* palette mode — a leading `>` in a Unified
    /// query upgrades it to CommandOnly without mutating the stored mode,
    /// so backspacing past the `>` naturally restores the unified view.
    pub(super) fn effective_palette_mode(mode: PaletteMode, query: &str) -> PaletteMode {
        if mode == PaletteMode::Unified && query.starts_with('>') {
            PaletteMode::CommandOnly
        } else {
            mode
        }
    }

    /// Strip the command-only `>` prefix (plus any following whitespace) when
    /// the effective mode was derived from that prefix.
    pub(super) fn palette_filter_query(mode: PaletteMode, query: &str) -> &str {
        match (mode, query.strip_prefix('>')) {
            (PaletteMode::CommandOnly, Some(rest)) => rest.trim_start(),
            _ => query,
        }
    }

    /// Build the set of command rows matching `filter_query`.
    ///
    /// Commands without a keybinding are intentionally still included — the
    /// palette is the primary access surface going forward, and some commands
    /// are expected to shed their hotkeys over time.
    pub(super) fn gather_command_entries(&self, filter_query: &str) -> Vec<CommandEntry> {
        self.ui_state
            .gather_command_entries(&self.config.keybindings, filter_query)
    }

    /// Build the mixed session+command list for the palette.
    ///
    /// Sessions first (when the effective mode is Unified), then commands.
    async fn build_palette_items(&self, mode: PaletteMode, query: &str) -> Vec<QuickSwitchItem> {
        let eff_mode = Self::effective_palette_mode(mode, query);
        let eff_query = Self::palette_filter_query(eff_mode, query);
        let mut out: Vec<QuickSwitchItem> = Vec::new();
        if let PaletteMode::SectionPicker { session_id } = eff_mode {
            return self.gather_section_picker_items(session_id, eff_query);
        }
        if eff_mode == PaletteMode::Unified {
            for m in self.gather_quick_switch_matches(eff_query).await {
                out.push(QuickSwitchItem::Session(m));
            }
        }
        for c in self.gather_command_entries(eff_query) {
            out.push(QuickSwitchItem::Command(c));
        }
        out
    }

    /// Build the section-picker rows for the move-to-section palette mode.
    /// Always includes an "Auto" entry first to clear any existing override,
    /// followed by the implicit "In Progress" catch-all.
    fn gather_section_picker_items(
        &self,
        session_id: SessionId,
        filter_query: &str,
    ) -> Vec<QuickSwitchItem> {
        let q = filter_query.to_lowercase();
        let mut out: Vec<QuickSwitchItem> = Vec::new();
        let auto_label = "Auto (clear override)".to_string();
        if q.is_empty() || auto_label.to_lowercase().contains(&q) {
            out.push(QuickSwitchItem::SectionMove {
                session_id,
                target: None,
                label: auto_label,
            });
        }
        let in_progress = crate::session::IN_PROGRESS;
        if q.is_empty() || in_progress.to_lowercase().contains(&q) {
            out.push(QuickSwitchItem::SectionMove {
                session_id,
                target: Some(in_progress.to_string()),
                label: in_progress.to_string(),
            });
        }
        for section in &self.config.sections {
            if !q.is_empty() && !section.name.to_lowercase().contains(&q) {
                continue;
            }
            out.push(QuickSwitchItem::SectionMove {
                session_id,
                target: Some(section.name.clone()),
                label: section.name.clone(),
            });
        }
        out
    }

    /// Re-filter the quick-switch matches based on the current query.
    /// Rebuilds from list_items so backspace can widen results.
    pub(super) fn refilter_quick_switch(&mut self) {
        // Snapshot the inputs we need so the closure borrow on self doesn't
        // conflict with the `&mut self.ui_state.modal` below.
        let (mode, query) = match &self.ui_state.modal {
            Modal::QuickSwitch { mode, query, .. } => (*mode, query.value().to_string()),
            _ => return,
        };

        let eff_mode = Self::effective_palette_mode(mode, &query);
        let eff_query = Self::palette_filter_query(eff_mode, &query);

        // Section picker: re-filter the section rows and stop — falling
        // through would replace them with command entries.
        if let PaletteMode::SectionPicker { session_id } = eff_mode {
            let section_items = self.gather_section_picker_items(session_id, eff_query);
            if let Modal::QuickSwitch {
                matches,
                selected_idx,
                scroll,
                ..
            } = &mut self.ui_state.modal
            {
                *matches = section_items;
                if *selected_idx >= matches.len() {
                    *selected_idx = matches.len().saturating_sub(1);
                }
                *scroll = 0;
                *scroll = adjust_list_scroll(*selected_idx, *scroll, LIST_MAX_VISIBLE);
            }
            return;
        }

        // Build the session rows synchronously from list_items so the refilter
        // can run without awaiting the store lock on every keystroke.
        let mut scored_sessions: Vec<(i64, QuickSwitchMatch)> = Vec::new();
        if eff_mode == PaletteMode::Unified {
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
                    SessionListItem::SectionHeader { .. } | SessionListItem::Spacer => {}
                }
            }

            for item in &self.ui_state.list_items {
                if let SessionListItem::Worktree {
                    id,
                    title,
                    branch,
                    status,
                    ..
                } = item
                {
                    // Score against title and branch; best field wins.
                    let score = [title.as_str(), branch.as_str()]
                        .iter()
                        .filter_map(|s| crate::fuzzy::fuzzy_score(s, eff_query))
                        .max();
                    let Some(score) = score else { continue };
                    let project_name = project_names.get(id).cloned().unwrap_or_default();
                    scored_sessions.push((
                        score,
                        QuickSwitchMatch {
                            session_id: *id,
                            title: title.clone(),
                            branch: branch.clone(),
                            project_name,
                            status: *status,
                        },
                    ));
                }
            }

            if eff_query.is_empty() {
                // Preserve tree order for empty queries.
            } else {
                scored_sessions
                    .sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
            }
        }
        let session_items: Vec<QuickSwitchItem> = scored_sessions
            .into_iter()
            .map(|(_, m)| QuickSwitchItem::Session(m))
            .collect();

        let command_items: Vec<QuickSwitchItem> = self
            .gather_command_entries(eff_query)
            .into_iter()
            .map(QuickSwitchItem::Command)
            .collect();

        if let Modal::QuickSwitch {
            matches,
            selected_idx,
            scroll,
            ..
        } = &mut self.ui_state.modal
        {
            *matches = session_items;
            matches.extend(command_items);

            if *selected_idx >= matches.len() {
                *selected_idx = matches.len().saturating_sub(1);
            }
            // Refilter collapses to a fresh window: reset to the top then
            // adjust so the (now-clamped) selection is still visible.
            *scroll = 0;
            *scroll = adjust_list_scroll(*selected_idx, *scroll, LIST_MAX_VISIBLE);
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
    pub(super) async fn handle_delete_session(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
        if let Some(session_id) = self.ui_state.selected_session_id {
            let title = self
                .session(SessionRef::local(session_id))
                .map(|s| s.title.clone());
            let retarget = super::state::stack_retarget_preview_from_snapshot(
                &self.local_view().snapshot,
                session_id,
            );
            self.ui_state.modal = Modal::Confirm {
                title: "Delete Session".to_string(),
                message: delete_confirm_message(
                    title.as_deref(),
                    retarget.as_ref().map(|(n, b)| (*n, b.as_str())),
                ),
                on_confirm: ConfirmAction::DeleteSession { session_id },
            };
        }
    }

    /// Sweep every project for sessions whose PR has merged on GitHub and
    /// open a single confirmation that names the count. No-op (with a
    /// transient status message) when nothing qualifies.
    pub(super) async fn handle_delete_merged_pr_sessions(&mut self) {
        let merged: Vec<(SessionId, String)> = self
            .local_view()
            .snapshot
            .sessions
            .iter()
            .filter(|s| s.pr_merged || s.pr_state == crate::git::PrState::Merged)
            .map(|s| (s.session_id, s.branch.clone()))
            .collect();

        if merged.is_empty() {
            self.ui_state.status_message = Some((
                "No sessions with merged PRs".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        }

        let count = merged.len();
        let preview: Vec<String> = merged
            .iter()
            .take(5)
            .map(|(_, b)| format!("  • {b}"))
            .collect();
        let more = if count > 5 {
            format!("\n  … and {} more", count - 5)
        } else {
            String::new()
        };
        let message = format!(
            "Delete {count} session(s) with merged PRs?\n\nBranches:\n{}{}\n\nThis will kill the tmux sessions and remove the worktrees.",
            preview.join("\n"),
            more,
        );
        let session_ids = merged.into_iter().map(|(id, _)| id).collect();

        self.ui_state.modal = Modal::Confirm {
            title: "Delete merged-PR sessions".to_string(),
            message,
            on_confirm: ConfirmAction::DeleteMergedPrSessions { session_ids },
        };
    }

    /// Remove a single session: capture cleanup data, mutate persistent
    /// state, refresh the list, and spawn the tmux/worktree teardown.
    ///
    /// Shared by the `DeleteSession` and `DeleteMergedPrSessions` confirm
    /// arms. Clears `selected_session_id` only when it matches the removed
    /// session — bulk callers leave the user's current selection alone.
    /// Delete a session without blocking the UI. `backend.delete_session` owns
    /// the whole teardown — kill tmux, remove the worktree, re-point stacked
    /// children onto the parent, and retarget their PRs — so this just clears
    /// the selection (if the focused row is going away) and spawns the call;
    /// the change feed refreshes the tree on completion and a failure surfaces
    /// as an error toast. Shared by the single and bulk delete confirmations.
    fn delete_session_immediately(&mut self, session_id: SessionId) {
        // When deleting the focused row, drop the selection now; the
        // change-feed refresh clamps the cursor once the session is gone.
        if self.ui_state.selected_session_id == Some(session_id) {
            self.ui_state.selected_session_id = None;
        }
        let backend = self.local_arc();
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            if let Err(e) = backend.delete_session(session_id).await {
                let _ = tx
                    .send(AppEvent::StateUpdate(StateUpdate::Error {
                        message: format!("Failed to delete session: {e}"),
                    }))
                    .await;
            }
        });
    }

    /// Whether the currently selected list item is a section header.
    pub(super) fn selected_item_is_section_header(&self) -> bool {
        self.ui_state
            .list_state
            .selected()
            .and_then(|idx| self.ui_state.list_items.get(idx))
            .is_some_and(|item| matches!(item, SessionListItem::SectionHeader { .. }))
    }

    /// Toggle collapse/expand for the section that contains the selected item.
    ///
    /// When the selected item is a section header, toggle that section directly.
    /// When the selected item is a project or worktree, walk backwards to find
    /// the nearest section header and toggle it.
    pub(super) async fn handle_toggle_section(&mut self) {
        if self.config.sections.is_empty() {
            return;
        }
        let Some(idx) = self.ui_state.list_state.selected() else {
            return;
        };

        let section_name = self.find_parent_section_name(idx);
        let Some(name) = section_name else {
            return;
        };

        if self.ui_state.collapsed_sections.contains(&name) {
            self.ui_state.collapsed_sections.remove(&name);
        } else {
            self.ui_state.collapsed_sections.insert(name.clone());
        }

        self.refresh_list_items().await;

        // After rebuilding the list, find the section header and select it.
        // This handles both collapse (selected child is now hidden) and expand
        // (keep focus on the header).
        for (i, item) in self.ui_state.list_items.iter().enumerate() {
            if let SessionListItem::SectionHeader { name: n, .. } = item
                && *n == name
            {
                self.ui_state.list_state.list_state.select(Some(i));
                break;
            }
        }

        self.update_selection();
        self.spawn_preview_update();
    }

    /// Walk backwards from `idx` to find the name of the nearest section header.
    fn find_parent_section_name(&self, idx: usize) -> Option<String> {
        for i in (0..=idx).rev() {
            if let Some(SessionListItem::SectionHeader { name, .. }) =
                self.ui_state.list_items.get(i)
            {
                return Some(name.clone());
            }
        }
        None
    }

    /// Open the "Move to section" palette for the selected session.
    /// The palette lists "Auto" plus one entry per configured `[[sections]]`;
    /// selecting "Auto" clears any override.
    pub(super) async fn handle_move_to_section(&mut self) {
        if self.config.sections.is_empty() {
            self.ui_state.status_message = Some((
                "No [[sections]] configured".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        }
        let Some(session_id) = self.ui_state.selected_session_id else {
            return;
        };
        let mode = PaletteMode::SectionPicker { session_id };
        let matches = self.gather_section_picker_items(session_id, "");
        self.ui_state.modal = Modal::QuickSwitch {
            mode,
            query: super::Input::default(),
            matches,
            selected_idx: 0,
            scroll: 0,
        };
    }

    /// Apply a manual section move chosen in the picker palette.
    /// `target = Some(name)` sets the override; `target = None` is the
    /// "Auto" entry, which must fully re-evaluate from the predicates
    /// rather than honour the forward-only rule that `apply_assignment`
    /// uses for the background poller.
    pub(super) async fn apply_section_move(
        &mut self,
        session_id: SessionId,
        target: Option<String>,
    ) {
        let _ = self.local_arc().set_section(session_id, target).await;
        self.refresh_local_view().await;
        self.refresh_list_items().await;

        // The session has moved to a new position in the rebuilt list. Keep it
        // selected by finding its new index and moving the cursor there, then
        // refresh the preview pane for the (unchanged) selection.
        if self.select_session_in_tree(session_id) {
            self.spawn_preview_update();
        }
    }

    /// Handle rename session - show input modal pre-filled with current title.
    /// Only the displayed title is changed; the underlying worktree, branch,
    /// and tmux session keep their original names.
    pub(super) async fn handle_rename_session(&mut self) {
        let Some(session_id) = self.ui_state.selected_session_id else {
            return;
        };
        let Some(current_title) = self
            .session(SessionRef::local(session_id))
            .map(|s| s.title.clone())
        else {
            return;
        };
        self.ui_state.modal = Modal::Input {
            title: "Rename Session".to_string(),
            prompt: "Enter new session name:".to_string(),
            value: current_title.into(),
            on_submit: InputAction::RenameSession { session_id },
            existing_branches: None,
            project_picker: None,
            program_picker: None,
            focus: super::InputFocus::Name,
            expanded: false,
        };
    }

    /// Create a session on the local backend off-thread. `create_session`
    /// commits the `Creating` placeholder early, so the change feed surfaces
    /// the new row immediately; `SessionCreated` (which selects it) or
    /// `SessionCreateFailed` completes the flow. On failure the backend removes
    /// its own half-created session.
    pub(super) fn spawn_create_session(&self, opts: crate::api::CreateSessionOpts) {
        let backend = self.local_arc();
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let update = match backend.create_session(opts).await {
                Ok(session_id) => StateUpdate::SessionCreated { session_id },
                Err(e) => StateUpdate::SessionCreateFailed {
                    message: format!("Failed to create session: {e}"),
                },
            };
            let _ = tx.send(AppEvent::StateUpdate(update)).await;
        });
    }

    /// Handle input modal submission. `program` is the command chosen in the
    /// new-session program picker, or `None` for flows without a picker (which
    /// then fall back to `default_program` inside the backend).
    pub(super) async fn handle_input_submit(
        &mut self,
        action: InputAction,
        value: String,
        program: Option<String>,
    ) {
        match action {
            InputAction::CreateSession {
                project_id,
                section,
            } => {
                if value.trim().is_empty() {
                    self.ui_state.status_message = Some((
                        "Session name cannot be empty".to_string(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                    return;
                }
                self.ui_state.modal = Modal::None;
                let Some(project_path) = self.project(project_id).map(|p| p.repo_path.clone())
                else {
                    self.ui_state.modal = Modal::Error {
                        message: "Project not found".to_string(),
                    };
                    return;
                };
                self.spawn_create_session(crate::api::CreateSessionOpts {
                    project_path,
                    title: value,
                    program,
                    initial_prompt: None,
                    effort: None,
                    mode: None,
                    base_branch: None,
                    section,
                    stack_parent: None,
                });
            }
            InputAction::CreateStackedSession {
                project_id,
                parent_session_id,
                parent_branch: _parent_branch,
            } => {
                if value.trim().is_empty() {
                    self.ui_state.status_message = Some((
                        "Session name cannot be empty".to_string(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                    return;
                }
                self.ui_state.modal = Modal::None;
                let Some(project_path) = self.project(project_id).map(|p| p.repo_path.clone())
                else {
                    self.ui_state.modal = Modal::Error {
                        message: "Project not found".to_string(),
                    };
                    return;
                };
                self.spawn_create_session(crate::api::CreateSessionOpts {
                    project_path,
                    title: value,
                    program,
                    initial_prompt: None,
                    effort: None,
                    mode: None,
                    base_branch: None,
                    section: None,
                    stack_parent: Some(parent_session_id),
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

                match self.local_arc().add_project(path).await {
                    Ok(project_id) => {
                        self.ui_state.status_message = Some((
                            format!("Added project {}", project_id),
                            Instant::now() + Duration::from_secs(3),
                        ));
                        self.refresh_local_view().await;
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
            InputAction::RenameSession { session_id } => {
                let new_title = value.trim().to_string();
                if new_title.is_empty() {
                    self.ui_state.status_message = Some((
                        "Session name cannot be empty".to_string(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                    return;
                }
                let _ = self.local_arc().rename_session(session_id, new_title).await;
                self.refresh_local_view().await;
                self.refresh_list_items().await;
            }
            InputAction::ScanDirectory => {
                let expanded = crate::tui::path_completer::expand_tilde(value.trim());
                let path = PathBuf::from(expanded);
                if !path.exists() {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Path does not exist: {}", path.display()),
                    };
                    return;
                }
                if !path.is_dir() {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Not a directory: {}", path.display()),
                    };
                    return;
                }

                // If the path itself is a git repo, just add it directly
                if path.join(".git").exists() {
                    match self.local_arc().add_project(path).await {
                        Ok(project_id) => {
                            self.ui_state.status_message = Some((
                                format!("Added project {}", project_id),
                                Instant::now() + Duration::from_secs(3),
                            ));
                            self.refresh_local_view().await;
                            self.refresh_list_items().await;
                            if let Some(idx) =
                                self.ui_state.list_items.iter().position(|item| {
                                    matches!(item, SessionListItem::Project { id, .. } if *id == project_id)
                                })
                            {
                                self.ui_state.list_state.select(Some(idx));
                            }
                        }
                        Err(e) => {
                            self.ui_state.modal = Modal::Error {
                                message: format!("Failed to add project: {}", e),
                            };
                        }
                    }
                    return;
                }

                // Show loading modal
                self.ui_state.modal = Modal::Loading {
                    title: "Scanning".to_string(),
                    message: format!("Scanning {} for git repos…", path.display()),
                    hint: None,
                };

                match self.local_arc().scan_directory(path.clone()).await {
                    Ok(result) => {
                        if result.added == 0 && result.skipped == 0 {
                            self.ui_state.modal = Modal::Error {
                                message: format!("No git repositories found in {}", path.display()),
                            };
                        } else {
                            self.ui_state.modal = Modal::None;
                            self.ui_state.status_message = Some((
                                format!(
                                    "Added {} project{} ({} already existed)",
                                    result.added,
                                    if result.added == 1 { "" } else { "s" },
                                    result.skipped,
                                ),
                                Instant::now() + Duration::from_secs(5),
                            ));
                            self.refresh_local_view().await;
                            self.refresh_list_items().await;
                        }
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Scan failed: {}", e),
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
                self.delete_session_immediately(session_id);
                self.ui_state.status_message = Some((
                    "Deleting session…".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
            ConfirmAction::DeleteMergedPrSessions { session_ids } => {
                let total = session_ids.len();
                for sid in session_ids {
                    self.delete_session_immediately(sid);
                }
                self.ui_state.status_message = Some((
                    format!("Deleting {total} merged-PR session(s)…"),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
            ConfirmAction::RestartSession { session_id } => {
                match self.local_arc().restart_session(session_id).await {
                    Ok(_) => {
                        self.ui_state.status_message = Some((
                            "Session restarted".to_string(),
                            Instant::now() + Duration::from_secs(3),
                        ));
                        self.refresh_local_view().await;
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
                // `backend.remove_project` owns the teardown — kill the project
                // shell + each session's tmux and remove every worktree, then
                // drop the project (and its sessions) from state. Spawn it so
                // the worktree removals never block the UI; the change feed
                // refreshes the tree on completion.
                self.ui_state.selected_project_id = None;
                self.ui_state.status_message = Some((
                    "Removing project…".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
                let backend = self.local_arc();
                let tx = self.event_loop.sender();
                tokio::spawn(async move {
                    if let Err(e) = backend.remove_project(project_id).await {
                        let _ = tx
                            .send(AppEvent::StateUpdate(StateUpdate::Error {
                                message: format!("Failed to remove project: {e}"),
                            }))
                            .await;
                    }
                });
            }
        }
    }
}

/// Best-effort flat list of branch names (local + remote-only with the
/// `origin/` prefix stripped) for the new-session dialog's existing-branch
/// hint. Returns `None` if the repo can't be opened — the dialog falls back
/// to no hint rather than failing.
pub(super) fn existing_branch_names(repo_path: &std::path::Path) -> Option<Vec<String>> {
    match load_branch_entries(repo_path) {
        Ok(entries) => Some(entries.into_iter().map(|e| e.local_name).collect()),
        Err(e) => {
            tracing::warn!(
                "Failed to load branches for new-session hint at {}: {}",
                repo_path.display(),
                e
            );
            None
        }
    }
}

/// Load the branch list for a repo path and convert each entry into
/// a `BranchEntry` suitable for the Checkout modal.
///
/// For branches that exist both locally and as remote tracking refs
/// we keep only the local entry — it's what we'd check out anyway.
pub(super) fn load_branch_entries(repo_path: &std::path::Path) -> Result<Vec<BranchEntry>> {
    let backend = crate::git::GitBackend::open(repo_path)?;
    let branches = backend.list_branches()?;

    let mut local_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entries: Vec<BranchEntry> = Vec::new();

    for (name, is_remote) in &branches {
        if !is_remote {
            local_names.insert(name.clone());
            entries.push(BranchEntry {
                local_name: name.clone(),
                display_name: name.clone(),
                is_remote: false,
            });
        }
    }

    for (name, is_remote) in &branches {
        if !is_remote {
            continue;
        }
        // "origin/foo" → local candidate "foo"
        let local = name
            .split_once('/')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_else(|| name.clone());
        if local_names.contains(&local) {
            // Already represented by the local branch — don't double-list.
            continue;
        }
        entries.push(BranchEntry {
            local_name: local,
            display_name: name.clone(),
            is_remote: true,
        });
    }

    Ok(entries)
}
