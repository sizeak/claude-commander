//! UI-triggered background fetches: preview/diff/shell data, the review-diff
//! re-compose, enriched-PR info, and AI summaries.
//!
//! These are spawned in response to user actions (selection change, pane
//! switch, hotkeys), never on a fixed tick, and they reach the data they need
//! **through the [`CommanderBackend`](crate::backend::CommanderBackend) trait**
//! — never the local `StateStore`/`SessionManager` directly. The periodic
//! refresh loops (agent-state polling, PR-status checks, project auto-pull,
//! cross-instance state-sync) that used to live here now run inside the service
//! ([`CommanderService::spawn_background_tasks`](crate::api::CommanderService::spawn_background_tasks));
//! their results reach the TUI as fresh snapshots via the backend change feed.

use super::*;

impl App {
    /// Spawn a background task to fetch preview/diff/shell data.
    ///
    /// The task runs in parallel with the main event loop so that
    /// keyboard input is never blocked by I/O. Results arrive as
    /// `StateUpdate::PreviewReady` events. The fetch goes through the backend
    /// trait, so a remote backend serves the same preview over the wire.
    pub(super) fn spawn_preview_update(&mut self) {
        // Skip if a fetch is already in flight (with 5s safety timeout)
        if let Some(spawned_at) = self.ui_state.preview_update_spawned_at {
            if spawned_at.elapsed() < Duration::from_secs(5) {
                return;
            }
            debug!("Preview update stale (>5s), spawning new one");
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

        self.ui_state.preview_update_spawned_at = Some(Instant::now());

        debug!(
            "Spawning preview update for session={:?} project={:?}",
            session_id, project_id
        );

        tokio::spawn(async move {
            let (preview_content, diff_info, shell_content) =
                fetch_preview_data(&backend, session_id, project_id).await;

            debug!(
                "Preview fetch complete, sending PreviewReady (preview_len={} diff_lines={})",
                preview_content.len(),
                diff_info.line_count
            );

            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::PreviewReady {
                    session_id,
                    project_id,
                    preview_content,
                    diff_info,
                    shell_content,
                }))
                .await;
        });
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
        let highlight = self.theme.mode == crate::tui::theme::ColorMode::TrueColor;
        let text_fg = self.theme.review_palette().text;

        tokio::spawn(async move {
            let refreshed = match backend
                .refresh_review_if_changed(session_id, prev_hash)
                .await
            {
                Ok(Some(snapshot)) => {
                    let crate::api::ReviewSnapshot {
                        base,
                        diff,
                        comments,
                        reviewed,
                        content_hash,
                    } = snapshot;
                    // The precompute is CPU-bound and synchronous; keep it off
                    // the async pool and hand the diff back with its segments.
                    let (diff, segments) = tokio::task::spawn_blocking(move || {
                        let segments =
                            super::review::precompute_review_caches(&diff, highlight, text_fg);
                        (diff, segments)
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
                        segments,
                        content_hash,
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

    /// Spawn background fetches for info pane data (enriched PR + AI summary).
    ///
    /// Only called from user-initiated actions (pane switch, selection change).
    /// Not called from background ticks to avoid unnecessary regeneration.
    pub(super) fn spawn_info_fetch(&mut self) {
        // Only relevant when the Info pane is active
        if self.ui_state.right_pane_view != RightPaneView::Info {
            return;
        }

        let Some(sref) = self.ui_state.selected_session_id else {
            return;
        };
        let session_id = sref.id;

        // Find the session's PR number and project repo path
        let session_info = self.ui_state.list_items.iter().find_map(|item| {
            if let SessionListItem::Worktree { id, pr_number, .. } = item {
                if *id == session_id {
                    Some(*pr_number)
                } else {
                    None
                }
            } else {
                None
            }
        });

        let Some(pr_number) = session_info.flatten() else {
            // No PR for this session — skip enriched PR fetch
            return;
        };

        // Spawn enriched PR fetch if not already cached for this session
        let needs_enriched = self
            .ui_state
            .enriched_pr
            .as_ref()
            .is_none_or(|(sid, _)| *sid != session_id);

        let backend_kind = self
            .backend(sref.backend)
            .map(|h| h.backend.descriptor().kind)
            .unwrap_or(crate::backend::BackendKind::Local);
        if should_fetch_enriched_pr(needs_enriched, self.ui_state.gh_available, backend_kind) {
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

            tokio::spawn(async move {
                let info = if let Some(repo_path) = repo_path {
                    fetch_enriched_pr(&repo_path, pr_number).await
                } else {
                    None
                };

                let _ = tx
                    .send(AppEvent::StateUpdate(StateUpdate::EnrichedPrReady {
                        session_id,
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
            if let Err(e) = crate::git::lfs::pull(&worktree_path).await {
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
/// counts. The TUI's diff pane + info-pane stat line render from this, so a
/// remote backend's [`PreviewData`](crate::api::PreviewData) drives them
/// identically.
fn diff_info_from_preview(diff_text: String, stats: Option<crate::api::DiffStat>) -> Arc<DiffInfo> {
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

/// Fetch preview/diff/shell data for the currently selected session or project
/// through the backend trait. Runs outside the main event loop so it never
/// blocks keyboard input. Mirrors the placeholder strings the old direct-manager
/// path produced (so the rendered output is unchanged).
pub(super) async fn fetch_preview_data(
    backend: &Arc<dyn crate::backend::CommanderBackend>,
    session_id: Option<SessionId>,
    project_id: Option<ProjectId>,
) -> (String, Arc<DiffInfo>, String) {
    let no_shell = || "No shell session. Press 's' to open one.".to_string();
    if let Some(sid) = session_id {
        match backend
            .preview(crate::api::PreviewTarget::Session {
                id: sid,
                lines: None,
            })
            .await
        {
            Ok(p) => {
                let crate::api::PreviewData {
                    pane,
                    diff_text,
                    stats,
                    shell,
                    ..
                } = p;
                (
                    pane.unwrap_or_else(|| "Unable to capture content".to_string()),
                    diff_info_from_preview(diff_text, stats),
                    shell.unwrap_or_else(no_shell),
                )
            }
            Err(e) => {
                debug!("fetch_preview_data: session preview error: {e}");
                (
                    "Unable to capture content".to_string(),
                    Arc::new(DiffInfo::empty()),
                    no_shell(),
                )
            }
        }
    } else if let Some(pid) = project_id {
        match backend
            .preview(crate::api::PreviewTarget::Project(pid))
            .await
        {
            Ok(p) => {
                let crate::api::PreviewData {
                    diff_text,
                    stats,
                    shell,
                    ..
                } = p;
                (
                    String::new(),
                    diff_info_from_preview(diff_text, stats),
                    shell.unwrap_or_else(no_shell),
                )
            }
            Err(e) => {
                debug!("fetch_preview_data: project preview error: {e}");
                (String::new(), Arc::new(DiffInfo::empty()), no_shell())
            }
        }
    } else {
        debug!("fetch_preview_data: no selection");
        (
            "Select a session to see preview".to_string(),
            Arc::new(DiffInfo::empty()),
            String::new(),
        )
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
    gh_available: bool,
    backend_kind: crate::backend::BackendKind,
) -> bool {
    needs_enriched && gh_available && backend_kind == crate::backend::BackendKind::Local
}

#[cfg(test)]
mod enriched_pr_gate_tests {
    use super::should_fetch_enriched_pr;
    use crate::backend::BackendKind;

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
