//! User actions: session selection, creation, deletion, editor/PR/shell interactions.

use super::*;

/// Which cascade entrypoint `run_cascade_action` should invoke.
#[derive(Debug, Clone, Copy)]
enum CascadeAction {
    Start,
    Resume,
}

/// Outcome of [`App::open_path_in_editor`], so each caller chooses how to
/// surface a non-launch (the session list uses a toast / error modal; the
/// review view keeps itself open and reports inline).
pub(super) enum EditorLaunch {
    /// A GUI editor was spawned, or a terminal editor was queued for launch.
    Launched,
    /// The owning backend can't drive the operator's local editor (remote
    /// session); carries the message to show.
    Unavailable(String),
    /// A hard failure (no editor configured, or the GUI spawn errored); carries
    /// the message to show.
    Failed(String),
}

/// Delete each `(owning-backend, session-id)` pair strictly one at a time on a
/// single task. Sessions in the same repo must not have their worktrees removed
/// concurrently — parallel `git worktree remove` in one repo races — so the
/// batch is sequential rather than one task per session. A per-session failure
/// is surfaced but does not abort the rest of the batch.
pub(super) async fn delete_sessions_in_sequence(
    deletes: Vec<(Arc<dyn CommanderBackend>, SessionId)>,
    tx: tokio::sync::mpsc::Sender<AppEvent>,
) {
    for (backend, sid) in deletes {
        if let Err(e) = backend.delete_session(sid).await {
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::Error {
                    message: format!("Failed to delete session: {e}"),
                }))
                .await;
        }
    }
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

/// Order quick-switch session matches newest-attached first, then by title.
/// Sessions never attached (`None`) sort to the bottom. Used for the
/// empty-query palette so the most-recently-used session is at the top,
/// mirroring the pinned "Recent" block and the in-tmux session picker.
pub(super) fn recency_then_title(a: &QuickSwitchMatch, b: &QuickSwitchMatch) -> std::cmp::Ordering {
    b.last_attached_at
        .cmp(&a.last_attached_at)
        .then_with(|| a.title.cmp(&b.title))
}

/// Order scored palette session matches in place. A non-empty query ranks by
/// fuzzy score (best first, title tiebreak); an empty query falls back to
/// most-recently-attached order via [`recency_then_title`]. Shared by both
/// palette build paths (`gather_quick_switch_matches` and
/// `refilter_quick_switch`) so their ordering can never drift apart — the
/// original recency bug was exactly these two branches diverging.
pub(super) fn sort_palette_matches(scored: &mut [(i64, QuickSwitchMatch)], query_is_empty: bool) {
    if query_is_empty {
        scored.sort_by(|a, b| recency_then_title(&a.1, &b.1));
    } else {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
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

/// The Restart-confirmation body. A local session's resume behaviour is
/// governed by this client's `resume_session` config, so the message can
/// promise `/resume` semantics. A remote session's resume behaviour lives in
/// the server's config, which this client can't read — so it gets neutral
/// wording naming the session and promising no resume.
pub(super) fn restart_confirm_message(
    is_local: bool,
    resume_session: bool,
    title: Option<&str>,
) -> String {
    if !is_local {
        let subject = match title {
            Some(title) => format!("\"{title}\""),
            None => "this session".to_string(),
        };
        return format!(
            "Restart session {subject}?\nThis will kill its tmux session and start a fresh one."
        );
    }
    if resume_session {
        "This will kill the current tmux session and start a fresh one.\nClaude will pick up where it left off via /resume.".to_string()
    } else {
        "This will kill the current tmux session and start a fresh one.\nIf you want to pick up where you left off, you can use /resume.".to_string()
    }
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
        let selected = self.ui_state.selected_session_id.map(|r| r.id);
        self.ui_state.list_items.iter().any(|item| {
            matches!(
                item,
                SessionListItem::Worktree { id, status, .. }
                if selected == Some(*id)
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
        let Some(sref) = self.ui_state.selected_session_id else {
            info!("No session selected");
            return;
        };
        // Validate against the cached snapshot so a non-attachable session
        // reports immediately; the backend revives a dead-but-attachable tmux
        // session when the attach actually runs.
        match self.session(sref).map(|s| s.status) {
            Some(status) if status.can_attach() => {
                // Clear unread when attaching. Fire-and-forget: on a remote
                // backend this is a POST with the client ceiling, and ordering
                // doesn't matter (the attach stamps MRU separately server-side),
                // so it must not block Enter-to-attach on the event loop.
                let backend = self.backend_for(sref);
                let id = sref.id;
                tokio::spawn(async move {
                    let _ = backend.mark_read(id).await;
                });
                self.ui_state.attach_request = Some(AttachTarget::Session {
                    session: sref,
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
        if let Some(sref) = self.ui_state.selected_session_id {
            // The backend creates the `-sh` pair on demand; a failure surfaces
            // as an error modal once the attach runs.
            self.ui_state.attach_request = Some(AttachTarget::Session {
                session: sref,
                kind: AttachKind::Shell,
            });
            self.ui_state.should_quit = true;
        } else if let Some((backend, project_id)) = self.ui_state.selected_project_id {
            // Project shells are a local tmux affordance; a remote backend can't
            // host one, and the local `project_shell_name` lookup would fail on a
            // remote project id with a confusing error.
            if !self.backend_arc(backend).capabilities().shell_toggle {
                self.ui_state.status_message = Some((
                    "Shell is not available for remote projects".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
                return;
            }
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
            .map(|s| std::path::PathBuf::from(&s.worktree_path));

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
        let (backend, path) = if let Some(sref) = self.ui_state.selected_session_id {
            (
                sref.backend,
                self.session(sref)
                    .map(|s| std::path::PathBuf::from(&s.worktree_path)),
            )
        } else if let Some((backend, project_id)) = self.ui_state.selected_project_id {
            (
                backend,
                self.view_for(backend)
                    .snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .map(|p| p.repo_path.clone()),
            )
        } else {
            return;
        };

        let Some(path) = path else {
            return;
        };

        // The session list surfaces a soft "unavailable" as a transient toast
        // and a hard failure as a blocking error modal.
        match self.open_path_in_editor(backend, path) {
            EditorLaunch::Launched => {}
            EditorLaunch::Unavailable(msg) => {
                self.ui_state.status_message = Some((msg, Instant::now() + Duration::from_secs(3)));
            }
            EditorLaunch::Failed(message) => {
                self.ui_state.modal = Modal::Error { message };
            }
        }
    }

    /// Launch the configured editor on a local `path` owned by `backend`. Shared
    /// by the session-list [`handle_open_in_editor`](Self::handle_open_in_editor)
    /// and the review view, so both honour the same capability gate, editor
    /// resolution, and GUI-vs-terminal launch behaviour. Returns the outcome so
    /// each caller can surface failures in the way that fits its surface (error
    /// modal vs. inline review status) rather than this helper picking one.
    pub(super) fn open_path_in_editor(
        &mut self,
        backend: BackendId,
        path: std::path::PathBuf,
    ) -> EditorLaunch {
        // The editor launches on a local path; a remote backend's worktree
        // lives on the server, so the path here would be meaningless.
        if !self.backend_arc(backend).capabilities().open_editor {
            return EditorLaunch::Unavailable(
                "Open in editor is not available for remote sessions".to_string(),
            );
        }

        let Some(editor) = self.config.resolve_editor() else {
            return EditorLaunch::Failed(
                "No editor configured. Set 'editor' in config.toml or \
                 set $VISUAL / $EDITOR."
                    .to_string(),
            );
        };

        if self.config.is_gui_editor(&editor) {
            // GUI editor: spawn detached, TUI stays up
            if let Err(e) = std::process::Command::new(&editor).arg(&path).spawn() {
                return EditorLaunch::Failed(format!("Failed to launch '{}': {}", editor, e));
            }
        } else {
            // Terminal editor: tear down TUI, run foreground, restore
            self.ui_state.editor_command = Some((editor, path));
            self.ui_state.should_quit = true;
        }
        EditorLaunch::Launched
    }

    /// Handle "open PR in browser" — looks up the selected session's
    /// `pr_url` and launches the OS default handler (`open` on macOS,
    /// `xdg-open` on Linux, `cmd /c start` on Windows).
    pub(super) async fn handle_open_pull_request(&mut self) {
        let Some(sref) = self.ui_state.selected_session_id else {
            return;
        };
        let pr_url = self.session(sref).and_then(|s| s.pr_url.clone());
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
    async fn new_project_picker(
        &self,
        backend: BackendId,
        default: ProjectId,
    ) -> super::ProjectPicker {
        let mut choices: Vec<super::ProjectChoice> = self
            .view_for(backend)
            .snapshot
            .projects
            .iter()
            .map(|p| super::ProjectChoice {
                id: p.id,
                name: p.name.clone(),
                repo_path: p.repo_path.clone(),
            })
            .collect();
        choices.sort_by_key(|a| a.name.to_lowercase());
        let mut picker = super::ProjectPicker::new(choices, default);
        // The existing-branch hint scans the repo locally; only meaningful for a
        // local project (a remote project's path lives on the server).
        picker.branch_hint_enabled = backend == LOCAL_BACKEND_ID;
        picker
    }

    /// For a remote `backend`, spawn a background task that loads its
    /// `create_options` and posts [`StateUpdate::NewSessionProgramsLoaded`] to
    /// patch the just-opened New Session modal's program picker with the
    /// harnesses that backend actually supports. No-op for the local backend
    /// (its picker comes from local config and is set synchronously) so the
    /// event loop never blocks on a remote `create_options` request.
    fn spawn_remote_program_picker(&self, backend: BackendId, project_id: ProjectId) {
        if backend == LOCAL_BACKEND_ID {
            return;
        }
        let backend = self.backend_arc(backend);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let Some(picker) = backend
                .create_options()
                .await
                .ok()
                .and_then(program_picker_from_options)
            else {
                // Query failed or offered no programs — leave the local fallback
                // picker the modal already shows.
                return;
            };
            let _ = tx
                .send(AppEvent::StateUpdate(
                    StateUpdate::NewSessionProgramsLoaded { project_id, picker },
                ))
                .await;
        });
    }

    pub(super) async fn handle_new_session(&mut self) {
        if let Some((backend, project_id)) = self.ui_state.selected_project_id {
            let repo_path = self
                .view_for(backend)
                .snapshot
                .projects
                .iter()
                .find(|p| p.id == project_id)
                .map(|p| p.repo_path.clone());
            // The existing-branch collision hint runs a local gix scan; only
            // compute it for a local project (a remote project's `repo_path` is
            // a server-side path this machine can't read).
            let existing_branches = if backend == LOCAL_BACKEND_ID {
                repo_path.and_then(|p| existing_branch_names(&p))
            } else {
                None
            };
            // Capture the section under the cursor now, so a background list
            // refresh while the modal is open can't change where the new
            // session lands.
            let section = self
                .ui_state
                .list_state
                .selected()
                .and_then(|idx| super::selection::section_at(&self.ui_state.list_items, idx));
            let project_picker = self.new_project_picker(backend, project_id).await;
            // Open with the local-config picker immediately; for a remote
            // backend, a background task swaps in its supported programs when
            // `create_options` returns, so the modal never waits on that request.
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
                mask: false,
            };
            self.spawn_remote_program_picker(backend, project_id);
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
        let Some(sref) = self.ui_state.selected_session_id else {
            self.ui_state.status_message = Some((
                "Select a session to stack on top of".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };
        let selected_session_id = sref.id;
        let resolved = {
            let snap = &self.view_for(sref.backend).snapshot;
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
                    Some((project_id, top.session_id, top.title.clone()))
                })
        };
        let Some((project_id, parent_session_id, parent_title)) = resolved else {
            return;
        };
        let repo_path = self
            .view_for(sref.backend)
            .snapshot
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.repo_path.clone());
        // Local-only hint: a remote project's `repo_path` is server-side.
        let existing_branches = if sref.backend == LOCAL_BACKEND_ID {
            repo_path.and_then(|p| existing_branch_names(&p))
        } else {
            None
        };
        // Open with the local-config picker immediately; for a remote backend a
        // background task swaps in its supported programs (keyed by project) so
        // the modal never waits on `create_options`.
        self.ui_state.modal = Modal::Input {
            title: format!("New Session Stacked on \"{}\"", parent_title),
            prompt: "Enter session name:".to_string(),
            value: super::Input::default(),
            on_submit: InputAction::CreateStackedSession {
                project_id,
                parent_session_id,
            },
            existing_branches,
            project_picker: None,
            program_picker: Some(self.new_program_picker()),
            focus: super::InputFocus::Name,
            expanded: false,
            mask: false,
        };
        self.spawn_remote_program_picker(sref.backend, project_id);
    }

    /// Handle `Cascade merge main` — walk to the base of the selected
    /// session's stack and merge main → base → each descendant. Pauses on
    /// the first conflict; surface the outcome as a status-message toast.
    pub(super) async fn handle_cascade_merge_main(&mut self) {
        let Some(sref) = self.ui_state.selected_session_id else {
            self.ui_state.status_message = Some((
                "Select a session in a stack to cascade from".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };
        self.run_cascade_action(sref, CascadeAction::Start);
    }

    /// Handle `Cascade resume` — continue a previously paused cascade.
    /// Cascade state is per-backend and a cascade started on a remote can
    /// pause there, so resolve which backend is paused (preferring the
    /// selection's backend when several are) rather than assuming local.
    pub(super) async fn handle_cascade_resume(&mut self) {
        let Some((backend_id, sid)) = self.paused_cascade_backend() else {
            self.ui_state.status_message = Some((
                "No cascade in progress".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };
        self.run_cascade_action(SessionRef::new(backend_id, sid), CascadeAction::Resume);
    }

    /// The backend (and paused-at session) whose cascade is paused, if any.
    /// When more than one backend has a paused cascade, prefer the one owning
    /// the current selection so the footer action acts where the user is.
    pub(super) fn paused_cascade_backend(&self) -> Option<(BackendId, SessionId)> {
        let paused: Vec<(BackendId, SessionId)> = self
            .backends
            .iter()
            .filter_map(|h| h.view.snapshot.cascade_paused.map(|sid| (h.id, sid)))
            .collect();
        if let Some(sref) = self.ui_state.selected_session_id
            && let Some(hit) = paused.iter().find(|(b, _)| *b == sref.backend)
        {
            return Some(*hit);
        }
        paused.first().copied()
    }

    /// Handle `Push stack` — push every branch in the selected session's
    /// stack to origin, in base→leaf order, on a background task.
    pub(super) fn handle_push_stack(&mut self) {
        let Some(sref) = self.ui_state.selected_session_id else {
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
        // its cached map. Records the outcome in the operation ledger. Route to
        // the backend that owns the session.
        let backend = self.backend_for(sref);
        let backend_id = sref.backend.0;
        let session_id = sref.id;
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let result = backend
                .push_stack(session_id)
                .await
                .map_err(|e| e.to_string());
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::PushStackFinished {
                    backend_id,
                    result,
                }))
                .await;
        });
    }

    pub(super) async fn handle_push_stack_finished(
        &mut self,
        backend_id: BackendId,
        result: std::result::Result<crate::api::OperationStatus, String>,
    ) {
        // Refresh off the event loop; BackendChanged re-renders the tree.
        self.spawn_backend_view_refresh(backend_id);
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

    /// Handle `Cascade abandon` — clear the paused state without merging,
    /// on whichever backend's cascade is paused (see
    /// [`paused_cascade_backend`](Self::paused_cascade_backend)).
    pub(super) fn handle_cascade_abandon(&mut self) {
        let Some((backend_id, _)) = self.paused_cascade_backend() else {
            self.ui_state.status_message = Some((
                "No cascade in progress".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };
        // Spawn so a slow/remote backend never blocks the event loop;
        // `CascadeAbandonFinished` refreshes the view and toasts on completion.
        let backend = self.backend_arc(backend_id);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let result = backend.cascade_abandon().await.map_err(|e| e.to_string());
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::CascadeAbandonFinished {
                    backend_id: backend_id.0,
                    result,
                }))
                .await;
        });
    }

    pub(super) fn handle_cascade_abandon_finished(
        &mut self,
        backend_id: BackendId,
        result: std::result::Result<(), String>,
    ) {
        let (msg, secs) = match result {
            Ok(()) => {
                // Refresh off the event loop; BackendChanged re-renders the tree.
                self.spawn_backend_view_refresh(backend_id);
                ("Cascade pause cleared".to_string(), 3)
            }
            Err(e) => (format!("Cascade abandon failed: {e}"), 5),
        };
        self.ui_state.status_message = Some((msg, Instant::now() + Duration::from_secs(secs)));
    }

    fn run_cascade_action(&mut self, sref: SessionRef, action: CascadeAction) {
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
        // the operation ledger, returning the recorded status. Route to the
        // backend that owns the session the cascade is anchored on.
        let backend = self.backend_for(sref);
        let backend_id = sref.backend.0;
        let session_id = sref.id;
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let result = match action {
                CascadeAction::Start => backend.cascade_merge(session_id).await,
                CascadeAction::Resume => backend.cascade_resume().await,
            };
            let result = result.map_err(|e| e.to_string());
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::CascadeFinished {
                    backend_id,
                    result,
                }))
                .await;
        });
    }

    pub(super) async fn handle_cascade_finished(
        &mut self,
        backend_id: BackendId,
        result: std::result::Result<crate::api::OperationStatus, String>,
    ) {
        // Refresh off the event loop; BackendChanged re-renders the tree.
        self.spawn_backend_view_refresh(backend_id);
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
    /// Loads the current branch list through the owning backend (local or
    /// remote) without a fetch, then kicks off a fetch-refresh in a background
    /// task so newly-pushed remote branches appear once the fetch lands.
    pub(super) async fn handle_checkout_branch(&mut self) {
        let Some((_backend, project_id)) = self.ui_state.selected_project_id else {
            self.ui_state.status_message = Some((
                "Select a project first (use N to add one)".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        };

        // Resolve the backend that owns the project and confirm the project is
        // present in its cached view — for both local and remote projects.
        let backend_id = self.backend_of_project(project_id);
        if !self
            .view_for(backend_id)
            .snapshot
            .projects
            .iter()
            .any(|p| p.id == project_id)
        {
            self.ui_state.modal = Modal::Error {
                message: "Project not found".to_string(),
            };
            return;
        }
        let backend = self.backend_arc(backend_id);

        // Open the modal immediately with an empty, spinning list — both the
        // initial (no-fetch) listing and the fetch-refresh run on background
        // tasks so a slow/remote backend never blocks the event loop. The repo
        // lives where the backend runs (locally in-process, remotely
        // server-side), so both listings go through the backend.
        self.ui_state.modal = Modal::CheckoutBranch {
            project_id,
            query: super::Input::default(),
            all_branches: Vec::new(),
            filtered: Vec::new(),
            selected_idx: 0,
            scroll: 0,
            fetching: true,
        };

        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            // Phase 1: the fast, no-fetch listing populates the modal while the
            // spinner stays up. Phase 2: the fetch-refresh replaces it and clears
            // the spinner. A failed listing yields an empty list rather than an
            // error modal — the user can retry or type a branch name.
            let initial = backend
                .list_branches(project_id, false)
                .await
                .map(pairs_from_branches)
                .unwrap_or_default();
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::CheckoutBranchesLoaded {
                    project_id,
                    branches: initial,
                }))
                .await;

            let fetched = backend
                .list_branches(project_id, true)
                .await
                .map(pairs_from_branches)
                .unwrap_or_default();
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::CheckoutFetchComplete {
                    project_id,
                    branches: fetched,
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

        // Resolve the project in its *owning* backend's snapshot — a remote
        // project isn't in the local view, so the local-only lookup would 404.
        let backend_id = self.backend_of_project(project_id);
        let Some(project_path) = self
            .view_for(backend_id)
            .snapshot
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.repo_path.clone())
        else {
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
        self.spawn_create_session(
            backend_id,
            crate::api::CreateSessionOpts {
                project_path,
                title: branch_name.clone(),
                program: None,
                initial_prompt: None,
                effort: None,
                mode: None,
                model: None,
                base_branch: Some(branch_name),
                section: None,
                stack_parent: None,
            },
        );
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
    /// empty queries fall back to most-recently-attached order.
    pub(super) async fn gather_quick_switch_matches(&self, query: &str) -> Vec<QuickSwitchMatch> {
        let mut scored: Vec<(i64, QuickSwitchMatch)> = Vec::new();

        // Every backend's sessions, not just local — the palette mirrors the
        // whole tree (and the refilter path already builds from `list_items`,
        // which spans all backends). `project_name` carries the session's own
        // project label, matching how the tree groups it; selection resolves
        // the owning backend by session id (`backend_of_session`).
        for handle in &self.backends {
            for session in &handle.view.snapshot.sessions {
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
                        last_attached_at: session.last_attached_at,
                    },
                ));
            }
        }

        sort_palette_matches(&mut scored, query.is_empty());
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
        if let PaletteMode::ProgramPicker { session_id } = eff_mode {
            return self.gather_program_picker_items(session_id, eff_query);
        }
        if eff_mode == PaletteMode::RemoteServerPicker {
            return self.gather_remote_server_picker_items(eff_query);
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

        // Program picker: same shape — re-filter the program rows and stop.
        if let PaletteMode::ProgramPicker { session_id } = eff_mode {
            let program_items = self.gather_program_picker_items(session_id, eff_query);
            if let Modal::QuickSwitch {
                matches,
                selected_idx,
                scroll,
                ..
            } = &mut self.ui_state.modal
            {
                *matches = program_items;
                if *selected_idx >= matches.len() {
                    *selected_idx = matches.len().saturating_sub(1);
                }
                *scroll = 0;
                *scroll = adjust_list_scroll(*selected_idx, *scroll, LIST_MAX_VISIBLE);
            }
            return;
        }

        // Remote-server picker: same shape — re-filter the server rows and stop.
        if eff_mode == PaletteMode::RemoteServerPicker {
            let server_items = self.gather_remote_server_picker_items(eff_query);
            if let Modal::QuickSwitch {
                matches,
                selected_idx,
                scroll,
                ..
            } = &mut self.ui_state.modal
            {
                *matches = server_items;
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
            // Attach-time lookup, sourced from the cached backend snapshots
            // (the `list_items` rows don't carry it). Drives the empty-query
            // recency ordering below.
            let mut last_attached: std::collections::HashMap<
                SessionId,
                chrono::DateTime<chrono::Utc>,
            > = std::collections::HashMap::new();
            for handle in &self.backends {
                for s in &handle.view.snapshot.sessions {
                    if let Some(at) = s.last_attached_at {
                        last_attached.insert(s.session_id, at);
                    }
                }
            }
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
                    // Recent-block rows duplicate real worktree rows, which are
                    // already scored below — skip them here.
                    SessionListItem::SectionHeader { .. }
                    | SessionListItem::ServerHeader { .. }
                    | SessionListItem::Spacer
                    | SessionListItem::RecentsHeader
                    | SessionListItem::RecentSession { .. } => {}
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
                            last_attached_at: last_attached.get(id).copied(),
                        },
                    ));
                }
            }

            // Empty query ranks by recency (newest attach first), matching the
            // pinned "Recent" block and the in-tmux session picker; a real query
            // ranks by fuzzy score.
            sort_palette_matches(&mut scored_sessions, eff_query.is_empty());
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
            && let Some((_backend, project_id)) = self.ui_state.selected_project_id
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
        if let Some(sref) = self.ui_state.selected_session_id {
            let title = self.session(sref).map(|s| s.title.clone());
            let message = restart_confirm_message(
                sref.backend == LOCAL_BACKEND_ID,
                self.config.resume_session,
                title.as_deref(),
            );
            self.ui_state.modal = Modal::Confirm {
                title: "Restart Session".to_string(),
                message,
                on_confirm: ConfirmAction::RestartSession {
                    session_id: sref.id,
                },
            };
        }
    }

    /// Toggle keep-alive on the selected session (opt out of / back into
    /// auto-hibernation). Non-destructive, so it applies immediately and
    /// reports via a transient status message.
    pub(super) async fn handle_toggle_keep_alive(&mut self) {
        let Some(session_id) = self.ui_state.selected_session_id else {
            return;
        };
        match self
            .backend_arc(session_id.backend)
            .toggle_keep_alive(session_id.id)
            .await
        {
            Ok(keep_alive) => {
                let msg = if keep_alive {
                    "Keep-alive on — session won't auto-hibernate"
                } else {
                    "Keep-alive off — idle session may auto-hibernate"
                };
                self.ui_state.status_message =
                    Some((msg.to_string(), Instant::now() + Duration::from_secs(3)));
                self.refresh_list_items().await;
            }
            Err(e) => {
                self.ui_state.status_message = Some((
                    format!("Failed to toggle keep-alive: {e}"),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
        }
    }

    /// Handle delete session - show confirmation
    pub(super) async fn handle_delete_session(&mut self) {
        if self.selected_session_is_creating() {
            return;
        }
        if let Some(sref) = self.ui_state.selected_session_id {
            let session_id = sref.id;
            let title = self.session(sref).map(|s| s.title.clone());
            let retarget = super::state::stack_retarget_preview_from_snapshot(
                &self.view_for(sref.backend).snapshot,
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

    /// Trigger a PR-metadata refresh on every connected backend. The local
    /// backend is always connected; a degraded remote is skipped, since its
    /// link is down and the request would only error. Each backend routes the
    /// request to where its PR polling actually happens (local loop / server).
    pub(super) fn refresh_pr_status_all(&self) {
        // Snapshot the connected backends, then fan out in one spawned task so a
        // slow/blocked remote POST doesn't stall the event loop. A degraded
        // remote is skipped (its link is down and the request would only error);
        // the local backend is always connected.
        let backends: Vec<Arc<dyn CommanderBackend>> = self
            .backends
            .iter()
            .filter(|handle| {
                handle.id == LOCAL_BACKEND_ID
                    || matches!(handle.view.connection, ConnectionState::Connected)
            })
            .map(|handle| handle.backend.clone())
            .collect();
        tokio::spawn(async move {
            for backend in backends {
                let _ = backend.request_pr_refresh().await;
            }
        });
    }

    /// Sweep every project for sessions whose PR has merged on GitHub and
    /// open a single confirmation that names the count. No-op (with a
    /// transient status message) when nothing qualifies.
    pub(super) async fn handle_delete_merged_pr_sessions(&mut self) {
        // Sweep every backend's view, not just local — a merged-PR session on a
        // remote server is just as eligible, and the delete path already routes
        // per-session via `backend_of_session`.
        let merged: Vec<(SessionId, String)> = self
            .backends
            .iter()
            .flat_map(|h| h.view.snapshot.sessions.iter())
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
        if self.ui_state.selected_session_id.map(|r| r.id) == Some(session_id) {
            self.ui_state.selected_session_id = None;
        }
        let backend = self.backend_arc(self.backend_of_session(session_id));
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
        let Some(session_id) = self.ui_state.selected_session_id.map(|r| r.id) else {
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

    /// Open the "Change program" palette for the selected session. The palette
    /// lists the owning backend's configured programs; selecting one confirms,
    /// then changes the session's program and relaunches it fresh.
    ///
    /// The picker is seeded from local config immediately (correct and final for
    /// a local session); for a remote-backed session a background task swaps in
    /// the server's supported programs when its `create_options` returns, so the
    /// palette never waits on that request.
    pub(super) fn handle_change_program(&mut self) {
        let Some(sref) = self.ui_state.selected_session_id else {
            return;
        };
        let session_id = sref.id;
        let current = self
            .session(sref)
            .map(|s| s.program.clone())
            .unwrap_or_default();
        self.ui_state.program_picker_choices = self.config.program_choices();
        self.ui_state.program_picker_current = current;
        let matches = self.gather_program_picker_items(session_id, "");
        self.ui_state.modal = Modal::QuickSwitch {
            mode: PaletteMode::ProgramPicker { session_id },
            query: super::Input::default(),
            matches,
            selected_idx: 0,
            scroll: 0,
        };
        self.spawn_remote_program_choices(sref.backend, session_id);
    }

    /// For a remote `backend`, spawn a background task that loads its supported
    /// programs and posts [`StateUpdate::ProgramChoicesLoaded`] to replace the
    /// change-program palette's local-config fallback. No-op for the local
    /// backend (its choices come from local config and are already final).
    fn spawn_remote_program_choices(&self, backend: BackendId, session_id: SessionId) {
        if backend == LOCAL_BACKEND_ID {
            return;
        }
        let backend = self.backend_arc(backend);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let Ok(opts) = backend.create_options().await else {
                // Query failed — leave the local fallback the palette already shows.
                return;
            };
            if opts.programs.is_empty() {
                return;
            }
            let choices = opts
                .programs
                .into_iter()
                .map(crate::config::ProgramEntry::from)
                .collect();
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::ProgramChoicesLoaded {
                    session_id,
                    choices,
                }))
                .await;
        });
    }

    /// Build the program-picker rows for the change-program palette mode from
    /// `program_picker_choices`, filtered by a label/command substring. The row
    /// matching the session's current program is flagged.
    pub(super) fn gather_program_picker_items(
        &self,
        session_id: SessionId,
        filter_query: &str,
    ) -> Vec<QuickSwitchItem> {
        let q = filter_query.to_lowercase();
        let current = &self.ui_state.program_picker_current;
        self.ui_state
            .program_picker_choices
            .iter()
            .filter(|e| {
                q.is_empty()
                    || e.label.to_lowercase().contains(&q)
                    || e.command.to_lowercase().contains(&q)
            })
            .map(|e| {
                let base = if e.label == e.command {
                    e.command.clone()
                } else {
                    format!("{} ({})", e.label, e.command)
                };
                let label = if e.command == *current {
                    format!("{base}  — current")
                } else {
                    base
                };
                QuickSwitchItem::ProgramChange {
                    session_id,
                    program: e.command.clone(),
                    label,
                }
            })
            .collect()
    }

    /// Handle "add remote server" — step 1 of the chained flow: prompt for a
    /// display name. URL and token follow; submission of the token step runs
    /// an async connection probe before writing config.
    pub(super) fn handle_add_remote_server(&mut self) {
        self.ui_state.modal = Modal::Input {
            title: "Add Remote Server".to_string(),
            prompt: "Server name (shown as the tree header):".to_string(),
            value: super::Input::default(),
            on_submit: InputAction::AddRemoteServerName,
            existing_branches: None,
            project_picker: None,
            program_picker: None,
            focus: super::InputFocus::Name,
            expanded: false,
            mask: false,
        };
    }

    /// Handle "remove remote server" — open the palette in remote-server
    /// picker mode. Not gated on the server count: an empty config just
    /// reports there's nothing to remove.
    pub(super) fn handle_remove_remote_server(&mut self) {
        if self.config.remote_servers.is_empty() {
            self.ui_state.status_message = Some((
                "No remote servers configured".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
            return;
        }
        let matches = self.gather_remote_server_picker_items("");
        self.ui_state.modal = Modal::QuickSwitch {
            mode: PaletteMode::RemoteServerPicker,
            query: super::Input::default(),
            matches,
            selected_idx: 0,
            scroll: 0,
        };
    }

    /// Build the remove-server picker rows: one per configured server,
    /// filtered by name/url substring.
    pub(super) fn gather_remote_server_picker_items(
        &self,
        filter_query: &str,
    ) -> Vec<QuickSwitchItem> {
        let q = filter_query.to_lowercase();
        self.config
            .remote_servers
            .iter()
            .filter(|s| {
                q.is_empty()
                    || s.name.to_lowercase().contains(&q)
                    || s.url.to_lowercase().contains(&q)
            })
            .map(|s| QuickSwitchItem::RemoteServerRemove {
                name: s.name.clone(),
                label: format!("{} ({})", s.name, s.url),
            })
            .collect()
    }

    /// Persist a new `[[remote_servers]]` entry to config.toml (validating the
    /// combined list first) and wire up its live backend handle. Returns a
    /// user-facing error string on failure so callers can toast/modal it.
    pub(super) fn add_remote_server_to_config(
        &mut self,
        server: crate::config::RemoteServerConfig,
    ) -> std::result::Result<(), String> {
        let old = self.config.remote_servers.clone();
        let mut cfg = self.service.read_config();
        cfg.remote_servers.push(server);
        cfg.validate_remote_servers().map_err(|e| e.to_string())?;
        self.service.update_config(cfg).map_err(|e| e.to_string())?;
        self.config = self.service.read_config();
        let new = self.config.remote_servers.clone();
        self.apply_remote_servers_reload(&old, &new);
        Ok(())
    }

    /// Remove a `[[remote_servers]]` entry by name from config.toml and drop
    /// its live backend handle (selection falls back to local).
    pub(super) fn remove_remote_server_from_config(
        &mut self,
        name: &str,
    ) -> std::result::Result<(), String> {
        let old = self.config.remote_servers.clone();
        let mut cfg = self.service.read_config();
        cfg.remote_servers.retain(|s| s.name != name);
        self.service.update_config(cfg).map_err(|e| e.to_string())?;
        self.config = self.service.read_config();
        let new = self.config.remote_servers.clone();
        self.apply_remote_servers_reload(&old, &new);
        Ok(())
    }

    /// Kick off the async connection probe for a candidate server: build a
    /// backend via the injected factory and fetch a workspace snapshot (which
    /// exercises reachability, auth, and reports the server's tmux health).
    /// The outcome arrives as [`StateUpdate::RemoteServerProbed`].
    pub(super) fn spawn_remote_server_probe(&mut self, server: crate::config::RemoteServerConfig) {
        self.ui_state.modal = Modal::Loading {
            title: "Add Remote Server".to_string(),
            message: format!("Testing connection to {}…", server.url),
            hint: None,
        };
        // Nonce ties the eventual result to THIS flow: a probe that lands
        // after the user dismissed the modal (and some other Loading modal
        // happens to be up) must be dropped, not written to config.
        self.probe_nonce = self.probe_nonce.wrapping_add(1);
        let nonce = self.probe_nonce;
        let factory = self.remote_factory.clone();
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let result = match factory(&server) {
                Ok(backend) => match backend.workspace_snapshot().await {
                    Ok(snap) => Ok(snap.server.tmux_ok),
                    Err(e) => Err(e.to_string()),
                },
                Err(e) => Err(e.to_string()),
            };
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::RemoteServerProbed {
                    nonce,
                    server,
                    result,
                }))
                .await;
        });
    }

    /// Apply a manual section move chosen in the picker palette.
    /// `target = Some(name)` sets the override; `target = None` is the
    /// "Auto" entry, which must fully re-evaluate from the predicates
    /// rather than honour the forward-only rule that `apply_assignment`
    /// uses for the background poller.
    pub(super) fn apply_section_move(&mut self, session_id: SessionId, target: Option<String>) {
        // Spawn the section move so a slow/degraded remote never blocks the event
        // loop; `SessionMutationApplied` refreshes the view/tree, reselects the
        // (relocated) session, and refreshes its preview.
        let backend_id = self.backend_of_session(session_id);
        let backend = self.backend_arc(backend_id);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let _ = backend.set_section(session_id, target).await;
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::SessionMutationApplied {
                    backend_id: backend_id.0,
                    session_id,
                }))
                .await;
        });
    }

    /// Handle rename session - show input modal pre-filled with current title.
    /// Only the displayed title is changed; the underlying worktree, branch,
    /// and tmux session keep their original names.
    pub(super) async fn handle_rename_session(&mut self) {
        let Some(sref) = self.ui_state.selected_session_id else {
            return;
        };
        let session_id = sref.id;
        let Some(current_title) = self.session(sref).map(|s| s.title.clone()) else {
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
            mask: false,
        };
    }

    /// Create a session on backend `backend_id` off-thread (local or remote).
    /// `create_session` commits the `Creating` placeholder early, so the change
    /// feed surfaces the new row immediately; `SessionCreated` (which selects it) or
    /// `SessionCreateFailed` completes the flow. On failure the backend removes
    /// its own half-created session.
    pub(super) fn spawn_create_session(
        &self,
        backend_id: BackendId,
        opts: crate::api::CreateSessionOpts,
    ) {
        let backend = self.backend_arc(backend_id);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let update = match backend.create_session(opts).await {
                Ok(session_id) => StateUpdate::SessionCreated {
                    session_id,
                    backend_id: backend_id.0,
                },
                Err(e) => StateUpdate::SessionCreateFailed {
                    message: format!("Failed to create session: {e}"),
                },
            };
            let _ = tx.send(AppEvent::StateUpdate(update)).await;
        });
    }

    /// Handle input modal submission. `program` is the command chosen in the
    /// new-session program picker, or `None` for flows without a picker (which
    /// then fall back to the first configured program inside `prepare_session`,
    /// on whichever backend owns the target project).
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
                let backend_id = self.backend_of_project(project_id);
                let Some(project_path) = self
                    .view_for(backend_id)
                    .snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .map(|p| p.repo_path.clone())
                else {
                    self.ui_state.modal = Modal::Error {
                        message: "Project not found".to_string(),
                    };
                    return;
                };
                self.spawn_create_session(
                    backend_id,
                    crate::api::CreateSessionOpts {
                        project_path,
                        title: value,
                        program,
                        initial_prompt: None,
                        effort: None,
                        mode: None,
                        model: None,
                        base_branch: None,
                        section,
                        stack_parent: None,
                    },
                );
            }
            InputAction::CreateStackedSession {
                project_id,
                parent_session_id,
            } => {
                if value.trim().is_empty() {
                    self.ui_state.status_message = Some((
                        "Session name cannot be empty".to_string(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                    return;
                }
                self.ui_state.modal = Modal::None;
                let backend_id = self.backend_of_project(project_id);
                let Some(project_path) = self
                    .view_for(backend_id)
                    .snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .map(|p| p.repo_path.clone())
                else {
                    self.ui_state.modal = Modal::Error {
                        message: "Project not found".to_string(),
                    };
                    return;
                };
                self.spawn_create_session(
                    backend_id,
                    crate::api::CreateSessionOpts {
                        project_path,
                        title: value,
                        program,
                        initial_prompt: None,
                        effort: None,
                        mode: None,
                        model: None,
                        base_branch: None,
                        section: None,
                        stack_parent: Some(parent_session_id),
                    },
                );
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

                // Deliberately local-only this phase: the path input's existence
                // check and completer both resolve against *this* machine's
                // filesystem, so the path is only meaningful on the local
                // backend. Remote add-project routing is deferred until there's
                // a server-side path completer to pick a remote path with.
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
                // Spawn the rename so a slow/degraded remote never blocks the
                // event loop; `SessionMutationApplied` refreshes the view/tree
                // and keeps the renamed session selected.
                let backend_id = self.backend_of_session(session_id);
                let backend = self.backend_arc(backend_id);
                let tx = self.event_loop.sender();
                tokio::spawn(async move {
                    let _ = backend.rename_session(session_id, new_title).await;
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::SessionMutationApplied {
                            backend_id: backend_id.0,
                            session_id,
                        }))
                        .await;
                });
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

                // Local-only this phase for the same reason as add-project above:
                // the scanned directory is a local filesystem path. Remote
                // scan/add routing is deferred until a server-side path picker
                // exists.
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
            InputAction::AddRemoteServerName => {
                let name = value.trim().to_string();
                if name.is_empty() {
                    self.ui_state.status_message = Some((
                        "Server name cannot be empty".to_string(),
                        Instant::now() + Duration::from_secs(3),
                    ));
                    self.handle_add_remote_server();
                    return;
                }
                if self.config.remote_servers.iter().any(|s| s.name == name) {
                    self.ui_state.status_message = Some((
                        format!("A remote server named \"{name}\" already exists"),
                        Instant::now() + Duration::from_secs(3),
                    ));
                    self.handle_add_remote_server();
                    return;
                }
                self.ui_state.modal = Modal::Input {
                    title: "Add Remote Server".to_string(),
                    prompt: format!("Server URL for \"{name}\" (e.g. http://host:7878):"),
                    value: "http://".into(),
                    on_submit: InputAction::AddRemoteServerUrl { name },
                    existing_branches: None,
                    project_picker: None,
                    program_picker: None,
                    focus: super::InputFocus::Name,
                    expanded: false,
                    mask: false,
                };
            }
            InputAction::AddRemoteServerUrl { name } => {
                let url = value.trim().to_string();
                // Validate the same way config loading does: a candidate list
                // containing just this entry catches empty/unparseable/hostless
                // URLs without duplicating the rules here.
                let mut candidate = self.service.read_config();
                candidate.remote_servers = vec![crate::config::RemoteServerConfig {
                    name: name.clone(),
                    url: url.clone(),
                    token: None,
                }];
                if let Err(e) = candidate.validate_remote_servers() {
                    self.ui_state.status_message = Some((
                        format!("Invalid URL: {e}"),
                        Instant::now() + Duration::from_secs(4),
                    ));
                    self.ui_state.modal = Modal::Input {
                        title: "Add Remote Server".to_string(),
                        prompt: format!("Server URL for \"{name}\" (e.g. http://host:7878):"),
                        value: url.into(),
                        on_submit: InputAction::AddRemoteServerUrl { name },
                        existing_branches: None,
                        project_picker: None,
                        program_picker: None,
                        focus: super::InputFocus::Name,
                        expanded: false,
                        mask: false,
                    };
                    return;
                }
                self.ui_state.modal = Modal::Input {
                    title: "Add Remote Server".to_string(),
                    prompt: "Bearer token (leave empty for --allow-no-auth servers):".to_string(),
                    value: super::Input::default(),
                    on_submit: InputAction::AddRemoteServerToken { name, url },
                    existing_branches: None,
                    project_picker: None,
                    program_picker: None,
                    focus: super::InputFocus::Name,
                    expanded: false,
                    mask: true,
                };
            }
            InputAction::AddRemoteServerToken { name, url } => {
                let token = value.trim();
                let server = crate::config::RemoteServerConfig {
                    name,
                    url,
                    token: (!token.is_empty()).then(|| token.to_string()),
                };
                self.spawn_remote_server_probe(server);
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
                // Drop selection now if the focused row is in the batch; the
                // change-feed refresh clamps the cursor once they're gone.
                if let Some(sel) = self.ui_state.selected_session_id.map(|r| r.id)
                    && session_ids.contains(&sel)
                {
                    self.ui_state.selected_session_id = None;
                }
                // Sequence the whole batch through ONE task: several of these
                // sessions can live in the same git repo, and concurrent
                // `git worktree remove` calls in one repo race. Per-session
                // routing to the owning backend is preserved.
                let deletes: Vec<(Arc<dyn CommanderBackend>, SessionId)> = session_ids
                    .iter()
                    .map(|sid| (self.backend_arc(self.backend_of_session(*sid)), *sid))
                    .collect();
                let tx = self.event_loop.sender();
                tokio::spawn(delete_sessions_in_sequence(deletes, tx));
                self.ui_state.status_message = Some((
                    format!("Deleting {total} merged-PR session(s)…"),
                    Instant::now() + Duration::from_secs(3),
                ));
            }
            ConfirmAction::RestartSession { session_id } => {
                // Spawn the restart so a slow/degraded remote never blocks the
                // event loop; `RestartFinished` refreshes the view and toasts.
                let backend_id = self.backend_of_session(session_id);
                self.ui_state.status_message = Some((
                    "Restarting session…".to_string(),
                    Instant::now() + Duration::from_secs(30),
                ));
                let backend = self.backend_arc(backend_id);
                let tx = self.event_loop.sender();
                tokio::spawn(async move {
                    let result = backend
                        .restart_session(session_id)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::RestartFinished {
                            backend_id: backend_id.0,
                            result,
                        }))
                        .await;
                });
            }
            ConfirmAction::ChangeProgram {
                session_id,
                program,
            } => {
                // Spawn the change+relaunch so a slow/degraded remote never blocks
                // the event loop; `RestartFinished` refreshes the view and toasts.
                let backend_id = self.backend_of_session(session_id);
                self.ui_state.status_message = Some((
                    format!("Changing program to {program}…"),
                    Instant::now() + Duration::from_secs(30),
                ));
                let backend = self.backend_arc(backend_id);
                let tx = self.event_loop.sender();
                tokio::spawn(async move {
                    let result = backend
                        .change_program(session_id, program)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::RestartFinished {
                            backend_id: backend_id.0,
                            result,
                        }))
                        .await;
                });
            }
            ConfirmAction::RemoveProject { project_id } => {
                // `backend.remove_project` owns the teardown — kill the project
                // shell + each session's tmux and remove every worktree, then
                // drop the project (and its sessions) from state. Spawn it so
                // the worktree removals never block the UI; the change feed
                // refreshes the tree on completion.
                let backend_id = self.backend_of_project(project_id);
                self.ui_state.selected_project_id = None;
                self.ui_state.status_message = Some((
                    "Removing project…".to_string(),
                    Instant::now() + Duration::from_secs(3),
                ));
                let backend = self.backend_arc(backend_id);
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
            ConfirmAction::AddRemoteServerAnyway { server } => {
                let name = server.name.clone();
                match self.add_remote_server_to_config(server) {
                    Ok(()) => {
                        self.ui_state.status_message = Some((
                            format!("Added remote server \"{name}\" (untested)"),
                            Instant::now() + Duration::from_secs(4),
                        ));
                        self.refresh_list_items().await;
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to save server: {e}"),
                        };
                    }
                }
            }
            ConfirmAction::RemoveRemoteServer { name } => {
                match self.remove_remote_server_from_config(&name) {
                    Ok(()) => {
                        self.ui_state.status_message = Some((
                            format!("Removed remote server \"{name}\""),
                            Instant::now() + Duration::from_secs(4),
                        ));
                        self.refresh_list_items().await;
                    }
                    Err(e) => {
                        self.ui_state.modal = Modal::Error {
                            message: format!("Failed to remove server: {e}"),
                        };
                    }
                }
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

/// Flatten a backend's `BranchInfo` list into the `(name, is_remote)` pairs the
/// Checkout state updates carry (and [`branch_entries_from_pairs`] consumes).
fn pairs_from_branches(infos: Vec<crate::api::BranchInfo>) -> Vec<(String, bool)> {
    infos.into_iter().map(|b| (b.name, b.is_remote)).collect()
}

/// Build a `ProgramPicker` from a backend's `create_options`, or `None` when the
/// options carry no programs (the caller falls back to the local-config picker).
fn program_picker_from_options(opts: crate::api::CreateOptions) -> Option<super::ProgramPicker> {
    if opts.programs.is_empty() {
        return None;
    }
    let selected = opts
        .programs
        .iter()
        .position(|e| e.command == opts.default_program)
        .unwrap_or(0);
    let choices = opts
        .programs
        .into_iter()
        .map(|p| crate::config::ProgramEntry {
            label: p.label,
            command: p.command,
        })
        .collect();
    Some(super::ProgramPicker { choices, selected })
}

/// Load the branch list for a repo path and convert each entry into
/// a `BranchEntry` suitable for the Checkout modal.
///
/// For branches that exist both locally and as remote tracking refs
/// we keep only the local entry — it's what we'd check out anyway.
pub(super) fn load_branch_entries(repo_path: &std::path::Path) -> Result<Vec<BranchEntry>> {
    let backend = crate::git::GitBackend::open(repo_path)?;
    Ok(branch_entries_from_pairs(backend.list_branches()?))
}

/// Convert raw `(name, is_remote)` branch pairs — as produced by
/// [`GitBackend::list_branches`](crate::git::GitBackend::list_branches) or a
/// backend's [`list_branches`](crate::backend::CommanderBackend::list_branches)
/// (`BranchInfo`) — into the `BranchEntry` list the Checkout modal shows.
///
/// For branches that exist both locally and as remote tracking refs we keep only
/// the local entry — it's what we'd check out anyway. The single source of truth
/// for this dedup, shared by the initial load and the post-fetch refresh.
pub(super) fn branch_entries_from_pairs(
    branches: impl IntoIterator<Item = (String, bool)>,
) -> Vec<BranchEntry> {
    let branches: Vec<(String, bool)> = branches.into_iter().collect();

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

    entries
}
