//! Background tasks: preview updates, PR status checks, info fetching, AI
//! summaries, cross-instance state sync, and agent-state polling.
//!
//! PHASE-D SEAM: this module is the one remaining place the TUI reaches past the
//! `CommanderBackend` trait into `self.service` (the local `StateStore` +
//! `SessionManager`/`TmuxExecutor`). Every `self.service.store()` /
//! `self.service.session_manager()` access in the crate now lives here, in the
//! loop-spawning methods below (`spawn_preview_update`, `spawn_pr_status_check`,
//! `spawn_info_fetch`, `spawn_ai_summary_if_needed`, `spawn_state_sync`,
//! `spawn_agent_poll`). Phase D moves this polling/refresh work behind the
//! backend (server-side for a remote backend, an in-process cache for the local
//! one) and a bulk agent-states cache, at which point these direct-service
//! clones and the transitional `App::service` field are deleted. Until then
//! they are deliberately confined to this module so the rest of the TUI is
//! already backend-only.

use futures::StreamExt;

use super::*;

/// Cap concurrent subprocess fan-outs (e.g. `gh pr list` across all
/// sessions). Each call holds 3+ pipe FDs, so unbounded fan-out can
/// EMFILE under the macOS launchd 256-FD default.
const PR_FANOUT_CONCURRENCY: usize = 8;

/// Cap concurrent project-branch pulls so a user with many projects
/// doesn't spawn one `git fetch` per project at the same instant.
const PROJECT_PULL_FANOUT_CONCURRENCY: usize = 4;

/// Minimum gap between PR-status fan-outs. Debounces rapid manual triggers
/// (e.g. double-enter on the palette "Refresh PR status" action) so we don't
/// launch several concurrent `gh pr list` sweeps at once. It sits far below
/// `pr_check_interval_secs`, so a manual refresh is still effectively immediate
/// — and the periodic caller already gates on that longer cadence, so this only
/// bites bursts.
const PR_CHECK_DEBOUNCE: Duration = Duration::from_secs(2);

/// Whether enough time has elapsed since the last PR-status check to spawn
/// another (see [`PR_CHECK_DEBOUNCE`]). `None` (never checked) always passes.
fn pr_check_debounce_passed(last_check: Option<Instant>, now: Instant, debounce: Duration) -> bool {
    last_check.is_none_or(|t| now.saturating_duration_since(t) >= debounce)
}

impl App {
    /// Spawn a background task to fetch preview/diff/shell data.
    ///
    /// The task runs in parallel with the main event loop so that
    /// keyboard input is never blocked by I/O. Results arrive as
    /// `StateUpdate::PreviewReady` events.
    pub(super) fn spawn_preview_update(&mut self) {
        // Skip if a fetch is already in flight (with 5s safety timeout)
        if let Some(spawned_at) = self.ui_state.preview_update_spawned_at {
            if spawned_at.elapsed() < Duration::from_secs(5) {
                return;
            }
            debug!("Preview update stale (>5s), spawning new one");
        }

        let session_id = self.ui_state.selected_session_id;
        let project_id = self.ui_state.selected_project_id;
        let mgr = self.service.session_manager().clone();
        let tx = self.event_loop.sender();

        self.ui_state.preview_update_spawned_at = Some(Instant::now());

        debug!(
            "Spawning preview update for session={:?} project={:?}",
            session_id, project_id
        );

        tokio::spawn(async move {
            let (preview_content, diff_info, shell_content) =
                fetch_preview_data(&mgr, session_id, project_id).await;

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

        let service = self.service.clone();
        let tx = self.event_loop.sender();
        let highlight = self.theme.mode == crate::tui::theme::ColorMode::TrueColor;
        let text_fg = self.theme.review_palette().text;

        tokio::spawn(async move {
            let refreshed = match service
                .refresh_review_if_changed(&session_id, prev_hash)
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

    /// Spawn a background task to check PR status for all sessions
    pub(super) fn spawn_pr_status_check(&mut self) {
        // Debounce rapid re-triggers so we don't fan out several concurrent
        // `gh pr list` sweeps (e.g. double-enter on the palette action). The
        // periodic caller already gates on the much longer
        // `pr_check_interval_secs`, so in practice this only collapses bursts.
        if !pr_check_debounce_passed(
            self.ui_state.last_pr_check,
            Instant::now(),
            PR_CHECK_DEBOUNCE,
        ) {
            return;
        }
        self.ui_state.last_pr_check = Some(Instant::now());

        let store = self.service.store().clone();
        let tx = self.event_loop.sender();

        tokio::spawn(async move {
            // Collect session info under a brief read lock
            let sessions_to_check: Vec<(SessionId, String, std::path::PathBuf)> = {
                let state = store.read().await;
                state
                    .sessions
                    .values()
                    .filter(|s| s.status != SessionStatus::Creating)
                    .filter_map(|s| {
                        let project = state.projects.get(&s.project_id)?;
                        Some((s.id, s.branch.clone(), project.repo_path.clone()))
                    })
                    .collect()
            };

            let results: Vec<_> = futures::stream::iter(sessions_to_check.into_iter().map(
                |(session_id, branch, repo_path)| async move {
                    let pr_result = check_pr_for_branch(&repo_path, &branch).await;
                    (session_id, pr_result)
                },
            ))
            .buffer_unordered(PR_FANOUT_CONCURRENCY)
            .collect()
            .await;

            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::PrStatusReady {
                    results,
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

        let Some(session_id) = self.ui_state.selected_session_id else {
            return;
        };

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

        if needs_enriched && self.ui_state.gh_available {
            let store = self.service.store().clone();
            let tx = self.event_loop.sender();

            tokio::spawn(async move {
                // Look up the project repo path
                let repo_path = {
                    let state = store.read().await;
                    state
                        .sessions
                        .get(&session_id)
                        .and_then(|s| state.projects.get(&s.project_id))
                        .map(|p| p.repo_path.clone())
                };

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

    /// Walk the project list, decide which projects are due for a pull
    /// this tick (respecting the `project_pull_interval_secs` cadence,
    /// a 5s startup grace, and the in-flight set), and dispatch them via
    /// `spawn_project_pulls`.
    pub(super) async fn maybe_spawn_project_pulls(&mut self) {
        // Honour a small grace period after startup so the first tick after
        // launch doesn't immediately hammer every project.
        const STARTUP_GRACE: Duration = Duration::from_secs(5);
        if self.ui_state.started_at.elapsed() < STARTUP_GRACE {
            return;
        }

        let interval = Duration::from_secs(self.config.project_pull_interval_secs);

        // Cheap global throttle: a project can become due at most once per
        // `interval`, so sweeping the project list (state lock + clone) more
        // often than that on every render tick is wasted work. The per-project
        // `last_project_pull` cadence still governs which projects actually run.
        if let Some(last) = self.ui_state.last_project_pull_sweep
            && last.elapsed() < interval
        {
            return;
        }
        self.ui_state.last_project_pull_sweep = Some(Instant::now());

        let projects: Vec<(ProjectId, std::path::PathBuf, String)> = {
            let state = self.service.store().read().await;
            state
                .projects
                .values()
                .map(|p| (p.id, p.repo_path.clone(), p.main_branch.clone()))
                .collect()
        };

        let mut due: Vec<(ProjectId, std::path::PathBuf, String)> = Vec::new();
        for (id, path, main) in projects {
            if self.ui_state.project_pull_in_flight.contains(&id) {
                continue;
            }
            let is_due = match self.ui_state.last_project_pull.get(&id) {
                Some(t) => t.elapsed() >= interval,
                None => true,
            };
            if is_due {
                self.ui_state.project_pull_in_flight.insert(id);
                due.push((id, path, main));
            }
        }

        self.spawn_project_pulls(due);
    }

    /// Spawn background fast-forward pulls for each project listed in
    /// `due`. Sends one `ProjectPullFinished` event per project as work
    /// completes. The caller is responsible for marking the projects as
    /// in-flight in `UiState` so we don't double-spawn.
    pub(super) fn spawn_project_pulls(&self, due: Vec<(ProjectId, std::path::PathBuf, String)>) {
        if due.is_empty() {
            return;
        }
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            futures::stream::iter(due.into_iter().map(|(project_id, repo_path, main_branch)| {
                let tx = tx.clone();
                async move {
                    let outcome = run_project_pull(&repo_path, &main_branch).await;
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::ProjectPullFinished {
                            project_id,
                            outcome,
                        }))
                        .await;
                }
            }))
            .buffer_unordered(PROJECT_PULL_FANOUT_CONCURRENCY)
            .for_each(|_| async {})
            .await;
        });
    }

    /// Spawn AI summary generation for the given session.
    ///
    /// Called from the `GenerateSummary` hotkey handler. Always generates
    /// (unless already in flight or AI is disabled). Computes a full branch
    /// diff (committed vs main + uncommitted) and pipes it into Claude.
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

        let store = self.service.store().clone();
        let model = self.config.ai_summary_model.clone();
        let tx = self.event_loop.sender();

        tokio::spawn(async move {
            let session_info = {
                let state = store.read().await;
                state.sessions.get(&session_id).and_then(|s| {
                    let project = state.projects.get(&s.project_id)?;
                    Some((s.worktree_path.clone(), project.main_branch.clone()))
                })
            };

            let result = if let Some((worktree_path, main_branch)) = session_info {
                let diff_text = crate::git::compute_branch_diff(&worktree_path, &main_branch).await;
                let new_hash = diff_hash(&diff_text);
                let summary_result = fetch_branch_summary(&diff_text, &model).await;
                (summary_result, new_hash)
            } else {
                (Err("Session not found".to_string()), 0)
            };

            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::AiSummaryReady {
                    session_id,
                    result: result.0,
                    diff_hash: result.1,
                }))
                .await;
        });
    }

    /// Spawn the cross-instance state-sync loop: periodically reloads the state
    /// file and emits [`StateUpdate::ExternalChange`] when another instance
    /// mutated it. No-op when `state_sync_interval_ms` is 0. (PHASE-D seam.)
    pub(super) fn spawn_state_sync(&self) {
        if self.config.state_sync_interval_ms == 0 {
            return;
        }
        let store = self.service.store().clone();
        let tx = self.event_loop.sender();
        let interval_ms = self.config.state_sync_interval_ms;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
            loop {
                interval.tick().await;
                match store.reload_if_changed().await {
                    Ok(true) => {
                        let _ = tx
                            .send(AppEvent::StateUpdate(StateUpdate::ExternalChange))
                            .await;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        debug!("State sync check failed: {}", e);
                    }
                }
            }
        });
    }

    /// Spawn the agent-state poll loop: on each tick detects the agent state of
    /// every running session (plus the commander) and emits
    /// [`StateUpdate::AgentStatesUpdated`] on any change. No-op when
    /// `agent_state_poll_interval_ms` is 0. (PHASE-D seam.)
    pub(super) fn spawn_agent_poll(&self) {
        if self.config.agent_state_poll_interval_ms == 0 {
            return;
        }
        let store = self.service.store().clone();
        let tx = self.event_loop.sender();
        let interval_ms = self.config.agent_state_poll_interval_ms;
        let tmux = self.service.session_manager().tmux.clone();
        // The commander is project-less and absent from `state.sessions`, so it
        // is polled separately. Enablement is restart-required: the poll task
        // and the footer chip share `commander_enabled_at_init` so the chip
        // can't disagree when the live config is toggled.
        let commander_enabled = self.commander_enabled_at_init;
        let commander_program = self.config.commander_program();
        let commander_tmux = tmux.clone();
        tokio::spawn(async move {
            let cache_ttl = Duration::from_millis(interval_ms.saturating_sub(500).max(500));
            let mut detector = AgentStateDetector::new(tmux, cache_ttl);
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
            let mut last_commander_running = false;
            loop {
                interval.tick().await;
                let mut sessions: Vec<(SessionId, String, String)> = {
                    let state = store.read().await;
                    state
                        .sessions
                        .values()
                        .filter(|s| s.status == SessionStatus::Running)
                        .map(|s| (s.id, s.tmux_session_name.clone(), s.program.clone()))
                        .collect()
                };
                let commander_running =
                    commander_enabled && crate::commander::is_running(&commander_tmux).await;
                if commander_running {
                    sessions.push((
                        crate::commander::commander_sentinel_id(),
                        crate::commander::COMMANDER_TMUX_NAME.to_string(),
                        commander_program.clone(),
                    ));
                }
                // Quiet path: nothing to detect and the commander's running
                // state is unchanged — skip the tick (no list rebuild).
                if poll_tick_can_skip(
                    sessions.is_empty(),
                    commander_running,
                    last_commander_running,
                ) {
                    continue;
                }
                let states: HashMap<SessionId, AgentState> = if sessions.is_empty() {
                    HashMap::new()
                } else {
                    detector.detect_all(&sessions).await
                };
                // Send on any real change: fresh states, or the commander
                // flipped (so its chip can turn on *and* off).
                if poll_tick_should_send(
                    states.is_empty(),
                    commander_running,
                    last_commander_running,
                ) {
                    last_commander_running = commander_running;
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::AgentStatesUpdated {
                            states,
                            commander_running,
                        }))
                        .await;
                }
            }
        });
    }
}

/// Fetch preview/diff/shell data for the currently selected session or project.
///
/// Runs outside the main event loop so it never blocks keyboard input.
pub(super) async fn fetch_preview_data(
    mgr: &SessionManager,
    session_id: Option<SessionId>,
    project_id: Option<ProjectId>,
) -> (String, Arc<DiffInfo>, String) {
    if let Some(sid) = session_id {
        // Check if session is still Creating (no tmux session to capture yet)
        let is_creating = {
            let state = mgr.store.read().await;
            state
                .get_session(&sid)
                .is_some_and(|s| s.status == SessionStatus::Creating)
        };
        if is_creating {
            return (
                "Creating session...".to_string(),
                Arc::new(DiffInfo::empty()),
                String::new(),
            );
        }

        debug!(
            "fetch_preview_data: fetching content/diff/shell for session {}",
            sid
        );
        let (preview_result, diff_result, shell_result) = tokio::join!(
            mgr.get_content(&sid),
            mgr.get_diff(&sid),
            mgr.get_shell_content(&sid),
        );

        let preview = preview_result.map(|c| c.content).unwrap_or_else(|e| {
            debug!("fetch_preview_data: get_content error: {}", e);
            "Unable to capture content".to_string()
        });
        let diff = diff_result.unwrap_or_else(|e| {
            debug!("fetch_preview_data: get_diff error: {}", e);
            Arc::new(DiffInfo::empty())
        });
        let shell = match shell_result {
            Ok(Some(c)) => c.content,
            Ok(None) => "No shell session. Press 's' to open one.".to_string(),
            Err(e) => {
                debug!("fetch_preview_data: get_shell_content error: {}", e);
                "No shell session. Press 's' to open one.".to_string()
            }
        };

        (preview, diff, shell)
    } else if let Some(pid) = project_id {
        debug!(
            "fetch_preview_data: fetching diff/shell for project {}",
            pid
        );
        let (diff_result, shell_result) = tokio::join!(
            mgr.get_project_diff(&pid),
            mgr.get_project_shell_content(&pid),
        );

        let diff = diff_result.unwrap_or_else(|e| {
            debug!("fetch_preview_data: get_project_diff error: {}", e);
            Arc::new(DiffInfo::empty())
        });
        let shell = match shell_result {
            Ok(Some(c)) => c.content,
            _ => "No shell session. Press 's' to open one.".to_string(),
        };

        (String::new(), diff, shell)
    } else {
        debug!("fetch_preview_data: no selection");
        (
            "Select a session to see preview".to_string(),
            Arc::new(DiffInfo::empty()),
            String::new(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_check_debounce_allows_first_check_and_blocks_bursts() {
        let base = Instant::now();
        let debounce = Duration::from_secs(2);

        // Never checked before → always allowed.
        assert!(pr_check_debounce_passed(None, base, debounce));

        // Re-trigger inside the window → blocked (the burst case).
        assert!(!pr_check_debounce_passed(
            Some(base),
            base + Duration::from_millis(500),
            debounce
        ));
        // Just shy of the threshold → still blocked.
        assert!(!pr_check_debounce_passed(
            Some(base),
            base + Duration::from_millis(1_999),
            debounce
        ));

        // At the threshold → allowed.
        assert!(pr_check_debounce_passed(
            Some(base),
            base + Duration::from_secs(2),
            debounce
        ));
        // Well past it (e.g. the periodic cadence) → allowed.
        assert!(pr_check_debounce_passed(
            Some(base),
            base + Duration::from_secs(120),
            debounce
        ));
    }
}
