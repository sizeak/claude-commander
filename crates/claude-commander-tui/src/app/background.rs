//! UI-triggered background fetches: preview/diff/shell data, the review-diff
//! re-compose, enriched-PR info, and AI summaries.
//!
//! These are spawned in response to user actions (selection change, pane
//! switch, hotkeys), never on a fixed tick, and they reach the data they need
//! **through the [`CommanderBackend`](claude_commander_core::backend::CommanderBackend) trait**
//! — never the local `StateStore`/`SessionManager` directly. The periodic
//! refresh loops (agent-state polling, PR-status checks, project auto-pull,
//! cross-instance state-sync) that used to live here now run inside the service
//! ([`CommanderService::spawn_background_tasks`](claude_commander_core::api::CommanderService::spawn_background_tasks));
//! their results reach the TUI as fresh snapshots via the backend change feed.

use super::*;

/// A remote session's marks poll runs once per this many local ones (~5s):
/// each is a round trip to the server, and an agent's marks don't need to
/// land within the second.
const REMOTE_MARKS_POLL_EVERY: u64 = 5;

impl App {
    /// Whether an Info surface is currently showing: the `i` modal, or the list
    /// views' right-pane Info tab. The enriched-PR and AI-summary fetches feed
    /// only those, so they are gated on this.
    pub(super) fn is_info_open(&self) -> bool {
        matches!(self.ui_state.modal, Modal::Info { .. }) || self.is_info_tab_showing()
    }

    /// Whether the right pane is currently on its Info tab. False on the board
    /// (no right pane) and whenever a capture tab is selected.
    fn is_info_tab_showing(&self) -> bool {
        self.ui_state.is_info_tab()
    }

    /// Whether anything on screen currently consumes preview data: the list
    /// views' right pane, or the Info modal's diffstat. False in board view with
    /// no Info modal, so no per-tick tmux/git traffic runs there.
    fn preview_data_wanted(&self) -> bool {
        !self.ui_state.view_mode.is_board() || self.is_info_open()
    }

    /// Spawn a background fetch of the one part of the selected session's (or
    /// project's) preview the visible surface shows: the pane capture, the
    /// shell capture, or the diff stats ([`PreviewRequest`]).
    ///
    /// Only acquire the resource used by the visible surface. Gated on something actually showing that data, and
    /// guarded by a 5s in-flight window so a slow backend can't queue fetches.
    /// The fetch goes through the backend trait, so a remote session's content
    /// arrives over the wire. Results arrive as [`StateUpdate::PreviewReady`].
    pub(super) fn spawn_preview_update(&mut self) {
        if !self.preview_data_wanted() {
            return;
        }
        // Skip if a fetch is already in flight (with 5s safety timeout).
        if let Some(spawned_at) = self.ui_state.preview_update_spawned_at {
            if spawned_at.elapsed() < Duration::from_secs(5) {
                return;
            }
            debug!("Preview update stale (>5s), spawning a new one");
        }

        // Preview reads from whichever backend owns the selection.
        let sel_session = self.ui_state.selected_session_id;
        let sel_project = self.ui_state.selected_project_id;
        let backend_id = sel_session
            .map(|r| r.backend)
            .or_else(|| sel_project.map(|(b, _)| b))
            .unwrap_or(LOCAL_BACKEND_ID);
        let session_id = sel_session.map(|r| r.id);
        let project_id = sel_project.map(|(_, p)| p);
        let backend = self.backend_arc(backend_id);
        let tx = self.event_loop.sender();
        let spawned_at = Instant::now();
        self.ui_state.preview_update_spawned_at = Some(spawned_at);

        let request = if self.is_info_open() || self.ui_state.right_pane_view == RightPaneView::Info
        {
            PreviewRequest::Stats
        } else if self.ui_state.right_pane_view == RightPaneView::Shell || session_id.is_none() {
            PreviewRequest::Shell
        } else {
            PreviewRequest::Pane
        };
        if let Some(task) = self.ui_state.preview_task.take() {
            task.abort();
        }
        self.ui_state.preview_task = Some(tokio::spawn(async move {
            let payload = fetch_preview(&backend, session_id, project_id, request).await;
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::PreviewReady {
                    spawned_at,
                    session_id,
                    project_id,
                    payload,
                }))
                .await;
        }));
    }

    /// Spawn a background re-compose of the open review diff. When the working
    /// tree's diff differs from what the view currently shows, the freshly
    /// parsed-and-warmed payload arrives as [`StateUpdate::ReviewRefreshed`] and
    /// is folded into the view in place (preserving cursor/scroll/focus); an
    /// unchanged diff just clears the in-flight guard. This keeps the review
    /// view live without the user leaving and re-opening it — triggered when the
    /// session's agent goes idle (it likely just acted on applied comments) or
    /// on a manual refresh keypress.
    ///
    /// `title` is carried only to populate [`ReviewPrepared`]; the in-place
    /// `refresh_diff` keeps the view's existing title.
    pub(super) fn spawn_review_refresh(
        &mut self,
        session_id: SessionId,
        title: String,
        prev_hash: u64,
        manual: bool,
    ) {
        // Coalesce: one refresh at a time. The idle poll and a manual press can
        // race; whichever spawns first wins, the other is dropped.
        if self.ui_state.review_refresh_in_flight {
            return;
        }
        self.ui_state.review_refresh_in_flight = true;

        let backend = self.backend_arc(self.backend_of_session(session_id));
        let tx = self.event_loop.sender();
        let highlight = self.theme.mode == claude_commander_core::term_caps::ColorMode::TrueColor;

        tokio::spawn(async move {
            let refreshed = match backend
                .refresh_review_if_changed(session_id, prev_hash)
                .await
            {
                Ok(Some(snapshot)) => {
                    let claude_commander_core::api::ReviewSnapshot {
                        base,
                        diff,
                        comments,
                        reviewed,
                        content_hash,
                        dropped_comments,
                        // The TUI lays out from the parsed model it already
                        // has; only a remote client re-parses the raw text.
                        raw: _,
                    } = snapshot;
                    // The precompute is CPU-bound and synchronous; keep it off
                    // the async pool and hand the diff back with its models.
                    let (diff, models) = tokio::task::spawn_blocking(move || {
                        let models = super::review::precompute_review_caches(&diff, highlight);
                        (diff, models)
                    })
                    .await
                    .expect("review refresh precompute task panicked");
                    Some(Box::new(ReviewPrepared {
                        session_id,
                        title,
                        base,
                        diff,
                        comments,
                        reviewed,
                        models,
                        content_hash,
                        dropped_comments,
                    }))
                }
                Ok(None) => None,
                Err(e) => {
                    debug!("Review refresh failed: {e}");
                    None
                }
            };
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::ReviewRefreshed {
                    refreshed,
                    manual,
                }))
                .await;
        });
    }

    /// Poll the open review's stored reviewed marks so marks set outside this
    /// view (the `review` CLI — typically an agent — or another frontend)
    /// appear without a reopen. One small store read, no diff compose, so it
    /// is cheap enough to run every second for a local session; a remote one
    /// costs an HTTP round trip, so it runs every [`REMOTE_MARKS_POLL_EVERY`]
    /// calls instead. Called once a second; `second` counts those calls. A
    /// failed poll is silent: the next one retries, and the view is never
    /// wrong, only late.
    pub(super) fn spawn_reviewed_marks_poll(&mut self, second: u64) {
        if self.ui_state.reviewed_poll_in_flight {
            return;
        }
        let Some((session_id, generation)) = self
            .open_review()
            .map(|state| (state.session_id, state.reviewed_gen))
        else {
            return;
        };
        let backend_id = self.backend_of_session(session_id);
        if backend_id != LOCAL_BACKEND_ID && !second.is_multiple_of(REMOTE_MARKS_POLL_EVERY) {
            return;
        }
        self.ui_state.reviewed_poll_in_flight = true;
        let backend = self.backend_arc(backend_id);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let marks = backend
                .reviewed_marks(session_id)
                .await
                .inspect_err(|e| debug!("Reviewed-marks poll failed: {e}"))
                .ok();
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::ReviewedMarksPolled {
                    session_id,
                    generation,
                    marks,
                }))
                .await;
        });
    }

    /// Spawn background fetches for Info modal data (enriched PR + AI summary).
    ///
    /// Gated on the Info modal being open (a no-op otherwise) and guarded
    /// against double-spawns: `update_selection` calls this, and it runs on
    /// every selection/list refresh, so without the in-flight guard an open Info modal
    /// would re-spawn a duplicate `gh` fetch on each input or refresh until the first resolves.
    pub(super) fn spawn_info_fetch(&mut self) {
        // Only relevant when the Info modal is open; a no-op otherwise.
        if !self.is_info_open() {
            return;
        }

        let Some(sref) = self.ui_state.selected_session_id else {
            return;
        };
        let session_id = sref.id;

        // Read the session's PR number from its backend snapshot (always
        // populated), not the board — the board is only built in board view.
        let pr_number = match self.session(sref) {
            Some(session) => session.pr_number,
            None => return,
        };

        let Some(pr_number) = pr_number else {
            // No PR for this session — skip enriched PR fetch
            return;
        };

        // Spawn enriched PR fetch if not already cached for this session and no
        // fetch is already in flight (with a 5s safety timeout, mirroring
        // `spawn_preview_update`).
        // Already cached, or already known to be unavailable for this session —
        // either way there is nothing to fetch. The unavailable marker matters
        // because an Info surface can now stay open indefinitely (the right
        // pane's tab, not just a briefly-open modal), and a failed fetch caches
        // nothing, so without it `gh` would be respawned every few seconds.
        let needs_enriched = self
            .ui_state
            .enriched_pr
            .as_ref()
            .is_none_or(|(sid, _)| *sid != session_id)
            && self.ui_state.enriched_pr_unavailable != Some(session_id);
        let in_flight = self
            .ui_state
            .enriched_pr_fetch_spawned_at
            .is_some_and(|spawned_at| spawned_at.elapsed() < Duration::from_secs(5));

        let backend_kind = self
            .backend(sref.backend)
            .map(|h| h.backend.descriptor().kind)
            .unwrap_or(claude_commander_core::backend::BackendKind::Local);
        let code_host = self
            .view_for(sref.backend)
            .snapshot
            .server
            .effective_code_host();
        if should_fetch_enriched_pr(needs_enriched, code_host.cli_available, backend_kind)
            && !in_flight
        {
            // Resolve the project's repo path from the cached snapshot rather
            // than the store — the backend seam owns the state.
            let snapshot = &self.view_for(sref.backend).snapshot;
            let repo_path = snapshot
                .sessions
                .iter()
                .find(|s| s.session_id == session_id)
                .and_then(|s| snapshot.projects.iter().find(|p| p.id == s.project_id))
                .map(|p| p.repo_path.clone());
            let tx = self.event_loop.sender();
            let spawned_at = Instant::now();
            self.ui_state.enriched_pr_fetch_spawned_at = Some(spawned_at);

            let code_host = code_host.host();
            let provider = code_host.provider;
            tokio::spawn(async move {
                let info = if let Some(repo_path) = repo_path {
                    claude_commander_core::git::hosting::fetch_enriched_review(
                        provider, &repo_path, pr_number,
                    )
                    .await
                } else {
                    None
                };

                let _ = tx
                    .send(AppEvent::StateUpdate(StateUpdate::EnrichedPrReady {
                        spawned_at,
                        session_id,
                        code_host,
                        info,
                    }))
                    .await;
            });
        }
    }

    /// Kick off a background `git lfs pull` for a session created with the
    /// LFS smudge skipped, so large files hydrate without blocking creation.
    /// Local sessions only: the worktree path in a remote session's snapshot
    /// is server-side, and the server host runs its own hydration.
    pub(super) async fn spawn_lfs_pull(&mut self, session_id: SessionId) {
        if !self.config.skip_lfs_smudge {
            return;
        }
        if !self.ui_state.lfs_pull_in_flight.insert(session_id) {
            return;
        }
        let worktree_path = self
            .local_view()
            .snapshot
            .sessions
            .iter()
            .find(|s| s.session_id == session_id)
            .map(|s| std::path::PathBuf::from(&s.worktree_path));
        let Some(worktree_path) = worktree_path else {
            self.ui_state.lfs_pull_in_flight.remove(&session_id);
            return;
        };
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            if let Err(e) = claude_commander_core::git::lfs::pull(&worktree_path).await {
                warn!(error = %e, "background git lfs pull failed");
            }
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::LfsPullFinished {
                    session_id,
                }))
                .await;
        });
    }

    /// Spawn AI summary generation for the given session.
    ///
    /// Called from the `GenerateSummary` hotkey handler. Always generates
    /// (unless already in flight or AI is disabled). The branch diff (committed
    /// vs main + uncommitted) is computed by the backend and piped into Claude.
    pub(super) fn spawn_ai_summary_if_needed(&mut self, session_id: SessionId) {
        if !self.config.ai_summary_enabled {
            return;
        }

        // Don't spawn if already in flight
        if matches!(
            self.ui_state.ai_summaries.get(&session_id),
            Some(AiSummary::Loading)
        ) {
            return;
        }

        self.ui_state
            .ai_summaries
            .insert(session_id, AiSummary::Loading);

        // Route to whichever backend owns the session; a remote backend serves
        // the branch diff over the wire (GET /api/sessions/{id}/branch-diff).
        let backend = self.backend_arc(self.backend_of_session(session_id));
        let model = self.config.ai_summary_model.clone();
        let tx = self.event_loop.sender();

        tokio::spawn(async move {
            let (result, new_hash) = match backend.branch_diff(session_id).await {
                Ok(diff_text) => {
                    let new_hash = diff_hash(&diff_text);
                    (fetch_branch_summary(&diff_text, &model).await, new_hash)
                }
                Err(e) => (Err(e.to_string()), 0),
            };

            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::AiSummaryReady {
                    session_id,
                    result,
                    diff_hash: new_hash,
                }))
                .await;
        });
    }
}

/// Reconstruct a [`DiffInfo`] from a preview's raw diff text and structured
/// counts. The Info modal's diffstat renders from this, so a remote backend's
/// [`PreviewData`](claude_commander_core::api::PreviewData) drives it identically to a local
/// one.
fn diff_info_from_preview(
    diff_text: String,
    stats: Option<claude_commander_core::api::DiffStat>,
) -> Arc<DiffInfo> {
    let line_count = diff_text.lines().count();
    Arc::new(DiffInfo {
        diff: diff_text,
        files_changed: stats.map_or(0, |s| s.files_changed),
        lines_added: stats.map_or(0, |s| s.lines_added),
        lines_removed: stats.map_or(0, |s| s.lines_removed),
        line_count,
        computed_at: Instant::now(),
        base_commit: String::new(),
    })
}

/// The part of the right pane / Info modal a preview fetch fetched — the
/// answer to one [`PreviewRequest`].
#[derive(Debug, Clone)]
pub enum PreviewPayload {
    /// Agent-pane capture (empty for a project, which has no agent pane).
    Pane(String),
    /// Shell-pane capture, or a placeholder when no shell session exists.
    Shell(String),
    /// Working-tree diff stats.
    Diff(Arc<DiffInfo>),
}

impl PreviewPayload {
    /// Replace the fetched part, leaving the others as they are, and report
    /// whether that changed what is shown — only the fetched part can have.
    pub(crate) fn apply(self, ui: &mut AppUiState) -> bool {
        match self {
            Self::Pane(pane) => replace(&mut ui.preview_content, pane),
            Self::Shell(shell) => replace(&mut ui.shell_content, shell),
            Self::Diff(diff) => {
                let changed = !ui.diff_info.same_content(&diff);
                ui.diff_info = diff;
                changed
            }
        }
    }
}

/// Store `new` in `slot`, reporting whether it differed.
fn replace(slot: &mut String, new: String) -> bool {
    let changed = *slot != new;
    *slot = new;
    changed
}

/// The one part of the preview a fetch asks for. Each maps to exactly one
/// [`PreviewPayload`] variant, so the wire's `PreviewPart::All` — which would
/// need all three — cannot be requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewRequest {
    /// The agent pane, for the list views' Preview tab.
    Pane,
    /// The shell pane.
    Shell,
    /// Diff stats only, for the Info tab and modal.
    Stats,
}

impl PreviewRequest {
    fn part(self) -> claude_commander_protocol::preview::PreviewPart {
        use claude_commander_protocol::preview::PreviewPart;
        match self {
            Self::Pane => PreviewPart::Pane,
            Self::Shell => PreviewPart::Shell,
            Self::Stats => PreviewPart::Stats,
        }
    }
}

/// Fetch the requested part of the selected session's or project's preview
/// through the backend trait — a remote selection's content arrives over the
/// wire. Runs outside the main event loop so it never blocks keyboard input.
async fn fetch_preview(
    backend: &Arc<dyn claude_commander_core::backend::CommanderBackend>,
    session_id: Option<SessionId>,
    project_id: Option<ProjectId>,
    request: PreviewRequest,
) -> PreviewPayload {
    let target = match (session_id, project_id) {
        (Some(id), _) => claude_commander_core::api::PreviewTarget::Session { id, lines: None },
        (None, Some(pid)) => claude_commander_core::api::PreviewTarget::Project(pid),
        (None, None) => {
            debug!("fetch_preview: no selection");
            // With nothing selected the shell pane is blank rather than
            // offering to open a shell. (A Pane request implies a session.)
            return match request {
                PreviewRequest::Shell => PreviewPayload::Shell(String::new()),
                PreviewRequest::Pane | PreviewRequest::Stats => preview_payload(request, None),
            };
        }
    };
    let fetched = backend.preview_part(target, request.part()).await;
    if let Err(e) = &fetched {
        debug!("fetch_preview: preview error: {e}");
    }
    preview_payload(request, fetched.ok())
}

/// The payload for `request` from a fetched preview, or from `None` when the
/// fetch failed — with the placeholder each pane renders when there is
/// nothing to show.
fn preview_payload(
    request: PreviewRequest,
    fetched: Option<claude_commander_core::api::PreviewData>,
) -> PreviewPayload {
    match request {
        PreviewRequest::Pane => PreviewPayload::Pane(
            fetched
                .and_then(|d| d.pane)
                .unwrap_or_else(|| "Unable to capture content".to_string()),
        ),
        PreviewRequest::Shell => PreviewPayload::Shell(
            fetched
                .and_then(|d| d.shell)
                .unwrap_or_else(|| "No shell session. Press 's' to open one.".to_string()),
        ),
        PreviewRequest::Stats => PreviewPayload::Diff(match fetched {
            Some(d) => diff_info_from_preview(d.diff_text, d.stats),
            None => Arc::new(DiffInfo::empty()),
        }),
    }
}

#[cfg(test)]
mod preview_payload_tests {
    use super::*;
    use claude_commander_core::api::{DiffStat, PreviewData};

    fn data() -> PreviewData {
        PreviewData {
            pane: Some("pane".into()),
            diff_text: "+a\n".into(),
            diff_stat: None,
            stats: Some(DiffStat {
                files_changed: 1,
                lines_added: 1,
                lines_removed: 0,
            }),
            shell: Some("shell".into()),
        }
    }

    fn text(p: PreviewPayload) -> String {
        match p {
            PreviewPayload::Pane(s) | PreviewPayload::Shell(s) => s,
            PreviewPayload::Diff(d) => panic!("expected text, got {d:?}"),
        }
    }

    #[test]
    fn a_fetched_preview_yields_the_requested_part() {
        assert!(matches!(
            preview_payload(PreviewRequest::Pane, Some(data())),
            PreviewPayload::Pane(p) if p == "pane"
        ));
        assert!(matches!(
            preview_payload(PreviewRequest::Shell, Some(data())),
            PreviewPayload::Shell(s) if s == "shell"
        ));
        let PreviewPayload::Diff(d) = preview_payload(PreviewRequest::Stats, Some(data())) else {
            panic!("Stats must yield the diff");
        };
        assert_eq!(
            (d.diff.as_str(), d.files_changed, d.lines_added),
            ("+a\n", 1, 1)
        );
    }

    #[test]
    fn a_failed_or_partial_fetch_falls_back_to_placeholders() {
        assert_eq!(
            text(preview_payload(PreviewRequest::Pane, None)),
            "Unable to capture content"
        );
        let no_parts = PreviewData {
            pane: None,
            shell: None,
            ..data()
        };
        assert_eq!(
            text(preview_payload(
                PreviewRequest::Pane,
                Some(no_parts.clone())
            )),
            "Unable to capture content"
        );
        for fetched in [None, Some(no_parts)] {
            assert_eq!(
                text(preview_payload(PreviewRequest::Shell, fetched)),
                "No shell session. Press 's' to open one."
            );
        }
        let PreviewPayload::Diff(d) = preview_payload(PreviewRequest::Stats, None) else {
            panic!("Stats must yield the diff");
        };
        assert!(d.same_content(&DiffInfo::empty()));
    }

    /// Painting reports whether it changed what is shown, comparing only the
    /// fetched part.
    #[test]
    fn apply_reports_a_change_to_its_own_part_only() {
        let shown = || AppUiState {
            preview_content: "pane".into(),
            shell_content: "shell".into(),
            ..Default::default()
        };
        let applies = |p: PreviewPayload| p.apply(&mut shown());
        assert!(!applies(PreviewPayload::Pane("pane".into())));
        assert!(applies(PreviewPayload::Pane("other".into())));
        assert!(!applies(PreviewPayload::Shell("shell".into())));
        assert!(applies(PreviewPayload::Shell("other".into())));
        // An equal recomputed diff is unchanged; a different one is a change.
        assert!(!applies(PreviewPayload::Diff(Arc::new(DiffInfo::empty()))));
        assert!(applies(PreviewPayload::Diff(Arc::new(DiffInfo {
            lines_added: 1,
            ..DiffInfo::empty()
        }))));
    }
}

/// Whether to spawn the local `gh` enriched-PR fetch for the selected session.
///
/// Only the local backend can shell out to `gh` against a project's on-disk
/// repo path; a remote session's repository lives server-side, so running `gh`
/// locally would query the wrong (or no) repo. For a remote session we skip the
/// wasted subprocess and leave the info pane showing the base PR data. Pure so
/// the gate is unit-testable without a live backend.
fn should_fetch_enriched_pr(
    needs_enriched: bool,
    cli_available: bool,
    backend_kind: claude_commander_core::backend::BackendKind,
) -> bool {
    needs_enriched
        && cli_available
        && backend_kind == claude_commander_core::backend::BackendKind::Local
}

#[cfg(test)]
mod enriched_pr_gate_tests {
    use super::should_fetch_enriched_pr;
    use claude_commander_core::backend::BackendKind;

    #[test]
    fn local_session_fetches_when_needed_and_available() {
        assert!(should_fetch_enriched_pr(true, true, BackendKind::Local));
    }

    #[test]
    fn remote_session_never_fetches() {
        // The load-bearing case: even with everything else satisfied, a remote
        // session must not spawn the local `gh` subprocess.
        assert!(!should_fetch_enriched_pr(true, true, BackendKind::Remote));
    }

    #[test]
    fn local_session_skips_when_gh_unavailable_or_not_needed() {
        assert!(!should_fetch_enriched_pr(false, true, BackendKind::Local));
        assert!(!should_fetch_enriched_pr(true, false, BackendKind::Local));
    }
}
