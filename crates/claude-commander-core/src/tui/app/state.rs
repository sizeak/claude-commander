//! State management: state updates, session sync, list refresh, selection persistence.

use super::*;
use crate::api::{ProjectInfo, SessionInfo, WorkspaceSnapshot};
// Used only by the tree-builder tests below, which build a snapshot from a
// hand-constructed `AppState` via the same projection production used before
// the change-feed cache landed.
#[cfg(test)]
use crate::api::workspace_snapshot_from_state;

impl App {
    pub(super) async fn handle_state_update(&mut self, update: StateUpdate) {
        match update {
            StateUpdate::BackendChanged {
                backend_id,
                snapshot,
                states,
            } => {
                let states = *states;
                let is_local = backend_id == crate::backend::LOCAL_BACKEND_ID.0;
                // Diff the OLD agent states (before we overwrite them) against
                // the fresh ones: if the session whose review is open just went
                // Working→Idle, it likely acted on applied comments — refresh the
                // review view in place.
                let review_refresh =
                    is_local.then(|| self.review_refresh_on_transition(&states.states));

                if let Some(handle) = self.backends.iter_mut().find(|h| h.id.0 == backend_id) {
                    handle.view.snapshot = *snapshot;
                    handle.view.agent_states = states.clone();
                    handle.view.connection = crate::backend::ConnectionState::Connected;
                }

                // The local backend drives the rendered agent-state map, the
                // commander chip, and the project-pull badges (folded out of the
                // snapshot the poll loops maintain). Single-backend this phase;
                // Phase E merges every backend's states into one tree.
                if is_local {
                    self.ui_state.agent_states = states.states;
                    self.ui_state.commander_running = states.commander_running;
                    self.apply_project_pull_badges();
                }
                if let Some(Some((sid, title, prev_hash))) = review_refresh {
                    self.spawn_review_refresh(sid, title, prev_hash, false);
                }
                self.refresh_list_items().await;
            }
            StateUpdate::BackendConnection { backend_id, state } => {
                if let Some(handle) = self.backends.iter_mut().find(|h| h.id.0 == backend_id) {
                    handle.view.connection = state;
                }
                // Re-render the tree so the server header reflects the new
                // health (and command gating re-evaluates).
                self.refresh_list_items().await;
            }
            StateUpdate::ContentUpdated { session_id, .. } => {
                debug!("Content updated for session {}", session_id);
            }
            StateUpdate::StatusChanged { session_id } => {
                debug!("Status changed for session {}", session_id);
                self.refresh_list_items().await;
            }
            StateUpdate::SessionAdded { session_id } => {
                debug!("Session added: {}", session_id);
                self.refresh_list_items().await;
            }
            StateUpdate::SessionRemoved { session_id } => {
                debug!("Session removed: {}", session_id);
                self.refresh_list_items().await;
            }
            StateUpdate::PreviewReady {
                session_id,
                project_id,
                preview_content,
                diff_info,
                shell_content,
            } => {
                let elapsed = self
                    .ui_state
                    .preview_update_spawned_at
                    .map(|t| t.elapsed())
                    .unwrap_or_default();
                self.ui_state.preview_update_spawned_at = None;

                // Only apply if selection hasn't changed since the fetch started
                // (compare ids; session/project ids are unique across backends)
                if session_id == self.ui_state.selected_session_id.map(|r| r.id)
                    && project_id == self.ui_state.selected_project_id.map(|(_, p)| p)
                {
                    debug!(
                        "Applying PreviewReady (preview_len={} diff_lines={} elapsed={:?})",
                        preview_content.len(),
                        diff_info.line_count,
                        elapsed
                    );
                    self.ui_state.preview_content = preview_content;
                    self.ui_state.diff_info = diff_info;
                    self.ui_state.shell_content = shell_content;
                } else {
                    debug!(
                        "Discarding stale PreviewReady (selection changed, elapsed={:?})",
                        elapsed
                    );
                }
            }
            StateUpdate::EnrichedPrReady { session_id, info } => {
                // Only apply if the session is still selected
                if self.ui_state.selected_session_id.map(|r| r.id) == Some(session_id) {
                    self.ui_state.enriched_pr = info.map(|pr| (session_id, pr));
                } else {
                    debug!("Discarding stale EnrichedPrReady for {}", session_id);
                }
            }
            StateUpdate::AiSummaryReady {
                session_id,
                result,
                diff_hash: hash,
            } => match result {
                Ok(text) => {
                    self.ui_state.ai_summaries.insert(
                        session_id,
                        AiSummary::Ready {
                            text,
                            diff_hash: hash,
                        },
                    );
                }
                Err(msg) => {
                    self.ui_state
                        .ai_summaries
                        .insert(session_id, AiSummary::Error(msg));
                }
            },
            StateUpdate::SessionCreated { session_id } => {
                debug!("Session created: {}", session_id);
                self.ui_state.modal = Modal::None;
                self.ui_state.status_message = Some((
                    format!("Created session {}", session_id),
                    Instant::now() + Duration::from_secs(3),
                ));
                self.reconcile_one_section_assignment(session_id).await;
                self.refresh_list_items().await;
                // Select the newly created session
                self.select_session_in_tree(session_id);
                self.spawn_preview_update();
            }
            StateUpdate::SessionCreateFailed { message } => {
                debug!("Session creation failed: {}", message);
                // The backend already removed its half-created session; the
                // change feed refreshes the tree. Just surface the error.
                self.ui_state.modal = Modal::Error { message };
            }
            StateUpdate::CheckoutFetchComplete {
                project_id: updated_project,
                branches,
            } => {
                // Only apply if the Checkout modal is still open for the
                // same project. Re-build the entry list and re-run the
                // current filter so the highlighted branch stays sensible.
                if let Modal::CheckoutBranch {
                    project_id,
                    all_branches,
                    fetching,
                    ..
                } = &mut self.ui_state.modal
                    && *project_id == updated_project
                {
                    *fetching = false;

                    // Reconstruct BranchEntry list from (name, is_remote) pairs,
                    // mirroring `load_branch_entries`'s dedup behavior.
                    let mut local_names: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
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
                        let local = name
                            .split_once('/')
                            .map(|(_, rest)| rest.to_string())
                            .unwrap_or_else(|| name.clone());
                        if local_names.contains(&local) {
                            continue;
                        }
                        entries.push(BranchEntry {
                            local_name: local,
                            display_name: name.clone(),
                            is_remote: true,
                        });
                    }

                    *all_branches = entries;
                    self.refilter_checkout_branches();
                }
            }
            StateUpdate::Error { message } => {
                self.ui_state.modal = Modal::Error { message };
            }
            StateUpdate::ReviewPrepared { prepared } => {
                // Only swap in the view if the loading spinner is still up. The
                // user can't navigate while it's shown, but another background
                // event could have replaced the modal (e.g. an error).
                if matches!(self.ui_state.modal, Modal::Loading { .. }) {
                    let ReviewPrepared {
                        session_id,
                        title,
                        base,
                        diff,
                        comments,
                        reviewed,
                        segments,
                        content_hash,
                    } = *prepared;
                    let mut state = DiffReviewState::new(session_id, title, base, diff, comments);
                    state.content_hash = content_hash;
                    state.reviewed = reviewed.into_iter().collect();
                    state.select_first_unreviewed();
                    state.prime_segments(segments);
                    self.reset_review_images();
                    self.ensure_review_image(&state).await;
                    self.ui_state.modal = Modal::ReviewDiff(Box::new(state));
                }
            }
            StateUpdate::ReviewImageLoaded {
                generation,
                path,
                side,
                image,
            } => {
                // Drop arrivals from a previous review: a stale fetch could
                // otherwise repopulate the cleared cache and show the wrong image
                // for a same-named path in the now-open review.
                if generation != self.review_image_gen.get() {
                    return;
                }
                // Build the render protocol on the main thread (it owns the
                // Picker) and cache it for the `&self` render path.
                let entry = match image {
                    Err(e) => ImageEntry::Failed(e),
                    Ok(img) => match &self.picker {
                        Some(picker) => {
                            let dynimg = std::sync::Arc::try_unwrap(img)
                                .unwrap_or_else(|shared| (*shared).clone());
                            ImageEntry::Ready(Box::new(picker.new_resize_protocol(dynimg)))
                        }
                        None => ImageEntry::Failed("terminal has no image support".to_string()),
                    },
                };
                self.review_images.borrow_mut().insert((path, side), entry);
            }
            StateUpdate::ReviewRefreshed { refreshed, manual } => {
                self.ui_state.review_refresh_in_flight = false;
                match refreshed {
                    Some(prepared) => {
                        // Fold the fresh diff in only if the same review is still
                        // open and the user isn't mid-comment (a rebuild would
                        // drop the draft); otherwise discard it.
                        if let Modal::ReviewDiff(state) = &mut self.ui_state.modal
                            && state.session_id == prepared.session_id
                            && state.comment.is_none()
                        {
                            let ReviewPrepared {
                                diff,
                                comments,
                                reviewed,
                                segments,
                                content_hash,
                                ..
                            } = *prepared;
                            state.refresh_diff(
                                diff,
                                comments,
                                reviewed.into_iter().collect(),
                                segments,
                                content_hash,
                            );
                            if manual {
                                self.set_review_status("Review refreshed");
                            }
                        }
                    }
                    None if manual => self.set_review_status("Review already up to date"),
                    None => {}
                }
            }
            StateUpdate::CascadeFinished { result } => {
                self.handle_cascade_finished(result).await;
            }
            StateUpdate::PushStackFinished { result } => {
                self.handle_push_stack_finished(result).await;
            }
            _ => {}
        }
    }

    /// Fold the workspace snapshot's per-project pull status into the render-side
    /// `project_pull_blocked` badge map. The background pull loop maintains the
    /// status server-side; only [`PullStatus::Blocked`] surfaces a badge (an
    /// advance/up-to-date/soft-fail clears any prior one).
    fn apply_project_pull_badges(&mut self) {
        use crate::api::PullStatus;
        self.ui_state.project_pull_blocked = self
            .local_view()
            .snapshot
            .project_pull
            .iter()
            .filter_map(|(id, status)| match status {
                PullStatus::Blocked { reason } => {
                    Some((*id, crate::git::BlockReason::from(*reason)))
                }
                _ => None,
            })
            .collect();
    }

    /// If the review view is open (and no comment draft is in progress) for a
    /// session that just transitioned Working→Idle between `self.ui_state`'s
    /// current agent states and `new_states`, return the arguments for an
    /// in-place review refresh.
    fn review_refresh_on_transition(
        &self,
        new_states: &HashMap<SessionId, AgentState>,
    ) -> Option<(SessionId, String, u64)> {
        let Modal::ReviewDiff(state) = &self.ui_state.modal else {
            return None;
        };
        if state.comment.is_some() {
            return None;
        }
        let sid = state.session_id;
        let was_working = self.ui_state.agent_states.get(&sid) == Some(&AgentState::Working);
        let now_idle = new_states.get(&sid) == Some(&AgentState::Idle);
        (was_working && now_idle).then(|| (sid, state.title.clone(), state.content_hash))
    }

    /// Re-run section assignment over every session against current config
    /// (after a live config change), then refresh the cached view + tree.
    pub(super) async fn reconcile_section_assignments(&mut self) {
        let _ = self.local_arc().reconcile_sections().await;
        self.refresh_local_view().await;
    }

    /// Re-run section assignment for a single freshly created session.
    pub(super) async fn reconcile_one_section_assignment(&mut self, session_id: SessionId) {
        let _ = self.local_arc().reconcile_one_section(session_id).await;
        self.refresh_local_view().await;
    }

    pub(super) async fn refresh_list_items(&mut self) {
        // Guard: if a section view is active but sections were removed from
        // config (hot-reload), fall back to ProjectGrouped.
        if matches!(
            self.ui_state.view_mode,
            ViewMode::SectionGrouped | ViewMode::SectionStacks
        ) && self.config.sections.is_empty()
        {
            self.ui_state.view_mode = ViewMode::ProjectGrouped;
        }

        // Read the change-feed-maintained cached snapshots. No store access on
        // the refresh path: a backend mutation bumps the change feed, whose task
        // fetches a fresh snapshot and folds it into the `BackendView` via
        // `StateUpdate::BackendChanged` before this runs. Each backend
        // contributes its own subtree under a per-server header — except when a
        // lone local backend is configured, where the header is suppressed so
        // the tree renders exactly as a single-machine setup always has.
        let single_backend = self.backends.len() == 1;
        let mut items: Vec<SessionListItem> = Vec::new();
        for handle in &self.backends {
            let snapshot = &handle.view.snapshot;
            let agent_states = &handle.view.agent_states.states;
            if !single_backend {
                items.push(SessionListItem::ServerHeader {
                    backend: handle.id,
                    name: handle.backend.descriptor().name,
                    connection: handle.view.connection.clone(),
                });
            }
            let mut backend_items = match self.ui_state.view_mode {
                ViewMode::ProjectGrouped => build_project_grouped_items(snapshot, agent_states),
                ViewMode::SectionGrouped => build_section_grouped_items(
                    snapshot,
                    &self.config.sections,
                    self.config.in_progress_limit,
                    agent_states,
                    &self.ui_state.collapsed_sections,
                ),
                ViewMode::SectionStacks => build_stacked_section_items(
                    snapshot,
                    &self.config.sections,
                    self.config.in_progress_limit,
                    agent_states,
                    &self.ui_state.collapsed_sections,
                ),
            };
            items.append(&mut backend_items);
        }

        let selectable: Vec<bool> = items.iter().map(|i| i.is_selectable()).collect();
        let group_starts: Vec<bool> = items.iter().map(|i| i.is_group_header()).collect();
        self.ui_state.list_items = items;
        // The footer's "resume cascade" hint shows if *any* backend is paused.
        self.ui_state.cascade_paused = self
            .backends
            .iter()
            .any(|h| h.view.snapshot.cascade_paused.is_some());
        if matches!(self.ui_state.view_mode, ViewMode::ProjectGrouped) {
            self.ui_state
                .list_state
                .set_item_count(self.ui_state.list_items.len());
        } else {
            self.ui_state.list_state.set_selectable(selectable);
        }
        self.ui_state.list_state.set_group_starts(group_starts);

        // Pre-compute stack chain for the selected session, from the snapshot of
        // the backend that owns it (stacks never span backends). Built into an
        // owned vec inside the snapshot-borrow scope, then moved into ui_state,
        // so the immutable borrow of `self` ends before the mutation.
        let stack_chain = self.ui_state.selected_session_id.map(|sref| {
            let snapshot = &self.view_for(sref.backend).snapshot;
            let session_id = sref.id;
            let by_id = session_index(snapshot);
            let mut entries: Vec<StackChainEntry> = Vec::new();
            if let Some(session) = by_id.get(&session_id).copied() {
                let project_sessions: Vec<&SessionInfo> = snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == session.project_id)
                    .map(|p| {
                        p.session_ids
                            .iter()
                            .filter_map(|sid| by_id.get(sid).copied())
                            .collect()
                    })
                    .unwrap_or_default();
                // Walk up to the stack base
                let mut base = session_id;
                for _ in 0..project_sessions.len() {
                    let base_session = project_sessions.iter().find(|s| s.session_id == base);
                    match base_session
                        .and_then(|s| crate::session::resolve_stack_parent(*s, &project_sessions))
                    {
                        Some(parent) => base = parent,
                        None => break,
                    }
                }
                let chain = crate::session::stack_chain_from_base(base, &project_sessions);
                if chain.len() > 1 {
                    for &sid in &chain {
                        if let Some(s) = by_id.get(&sid).copied() {
                            entries.push(StackChainEntry {
                                title: s.title.clone(),
                                status: s.status,
                                is_current: sid == session_id,
                            });
                        }
                    }
                }
            }
            entries
        });
        self.ui_state.stack_chain = stack_chain.unwrap_or_default();
    }

    /// Save current selection to persisted UI prefs, qualified by the owning
    /// backend's name so it survives a config reorder.
    pub(super) async fn save_selection(&self) {
        let session = self.ui_state.selected_session_id;
        let project = self.ui_state.selected_project_id;
        let backend_id = session
            .map(|r| r.backend)
            .or_else(|| project.map(|(b, _)| b));
        let backend_name =
            backend_id.and_then(|id| self.backend(id).map(|h| h.backend.descriptor().name));
        self.tui_prefs
            .set_selection(session.map(|r| r.id), project.map(|(_, p)| p), backend_name)
            .await;
    }

    /// Save left pane width to persisted UI prefs
    pub(super) async fn save_left_pane_pct(&self) {
        self.tui_prefs
            .set_left_pane_pct(self.ui_state.left_pane_pct)
            .await;
    }

    /// Restore selection and UI preferences from persisted UI prefs
    pub(super) async fn restore_selection(&mut self) {
        let prefs = self.tui_prefs.prefs();
        let (last_session, last_project, left_pane_pct) = (
            prefs.last_selected_session,
            prefs.last_selected_project,
            prefs.left_pane_pct,
        );

        if let Some(pct) = left_pane_pct {
            self.ui_state.left_pane_pct = pct.clamp(MIN_LEFT_PANE_PCT, MAX_LEFT_PANE_PCT);
        }

        // Try to find the last selected session or project in the list
        let target_idx = self.ui_state.list_items.iter().position(|item| match item {
            SessionListItem::Worktree { id, .. } => last_session.is_some_and(|s| s == *id),
            SessionListItem::Project { id, .. } => {
                last_session.is_none() && last_project.is_some_and(|p| p == *id)
            }
            SessionListItem::SectionHeader { .. }
            | SessionListItem::ServerHeader { .. }
            | SessionListItem::Spacer => false,
        });

        if let Some(idx) = target_idx {
            self.ui_state.list_state.select(Some(idx));
        } else if !self.ui_state.list_items.is_empty() {
            self.ui_state.list_state.select(Some(0));
        }
    }
}

/// Compute the display order of a project's sessions, grouping each stack
/// directly under its base.
///
/// Returns `(session_id, stacked_child)` pairs in the order they should appear.
/// Root-list sessions (unstacked + stack bases) are sorted newest-first by
/// `created_at`; stacked children follow their root in parent→child (stack
/// position) order at the single deeper indent.
pub(super) fn build_session_order<S: crate::session::SessionNode>(
    sessions: &[&S],
) -> Vec<(SessionId, bool)> {
    let mut root_sessions: Vec<&S> = Vec::new();
    let mut children_by_parent: HashMap<SessionId, Vec<&S>> = HashMap::new();
    for s in sessions {
        match crate::session::resolve_stack_parent(*s, sessions) {
            Some(parent_id) => {
                children_by_parent.entry(parent_id).or_default().push(s);
            }
            None => {
                root_sessions.push(s);
            }
        }
    }

    root_sessions.sort_by_key(|s| std::cmp::Reverse(s.node_created_at()));
    for children in children_by_parent.values_mut() {
        children.sort_by_key(|s| s.node_created_at());
    }

    let mut out = Vec::new();
    for root in root_sessions {
        out.push((root.node_id(), false));
        // to_visit is a LIFO stack; reverse the initial children and every
        // subsequent children-of-children push so pop() yields them in
        // ascending created_at order.
        let mut to_visit: Vec<&S> = children_by_parent
            .get(&root.node_id())
            .cloned()
            .unwrap_or_default();
        to_visit.reverse();
        while let Some(next) = to_visit.pop() {
            out.push((next.node_id(), true));
            if let Some(grandchildren) = children_by_parent.get(&next.node_id()) {
                for gc in grandchildren.iter().rev() {
                    to_visit.push(gc);
                }
            }
        }
    }
    out
}

fn worktree_item(
    session: &SessionInfo,
    agent_states: &HashMap<SessionId, AgentState>,
    project_name_prefix: Option<&str>,
    stacked_child: bool,
) -> SessionListItem {
    let title = match project_name_prefix {
        Some(prefix) => format!("{}/{}", prefix, session.title),
        None => session.title.clone(),
    };
    SessionListItem::Worktree {
        id: session.session_id,
        project_id: session.project_id,
        title,
        branch: session.branch.clone(),
        status: session.status,
        program: session.program.clone(),
        pr_number: session.pr_number,
        pr_url: session.pr_url.clone(),
        pr_merged: session.pr_merged,
        // The DTO carries the already-effective PR state; the list item's
        // renderer re-applies `effective_pr_state`, which is idempotent on
        // `Some`, so wrapping preserves the previous rendering exactly.
        pr_state: Some(session.pr_state),
        pr_draft: session.pr_draft,
        pr_labels: session.pr_labels.clone(),
        worktree_path: session.worktree_path.clone(),
        created_at: session.created_at,
        agent_state: agent_states.get(&session.session_id).copied(),
        unread: session.unread,
        stacked_child,
    }
}

/// Index a snapshot's sessions by id for O(1) lookup during tree building.
fn session_index(snapshot: &WorkspaceSnapshot) -> HashMap<SessionId, &SessionInfo> {
    snapshot
        .sessions
        .iter()
        .map(|s| (s.session_id, s))
        .collect()
}

pub(super) fn build_project_grouped_items(
    snapshot: &WorkspaceSnapshot,
    agent_states: &HashMap<SessionId, AgentState>,
) -> Vec<SessionListItem> {
    let by_id = session_index(snapshot);
    let mut items = Vec::new();
    let mut projects: Vec<&ProjectInfo> = snapshot.projects.iter().collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name));

    for project in projects {
        items.push(SessionListItem::Project {
            id: project.id,
            name: project.name.clone(),
            repo_path: project.repo_path.clone(),
            main_branch: project.main_branch.clone(),
            worktree_count: project.session_ids.len(),
            nested: false,
        });

        // Use stack-aware ordering so stacked children render indented
        // directly beneath their stack base.
        let sessions: Vec<&SessionInfo> = project
            .session_ids
            .iter()
            .filter_map(|sid| by_id.get(sid).copied())
            .collect();
        for (sid, stacked_child) in build_session_order(&sessions) {
            if let Some(session) = by_id.get(&sid).copied() {
                items.push(worktree_item(session, agent_states, None, stacked_child));
            }
        }
    }
    items
}

/// Resolve the advisory WIP limit for a section by name. Returns the
/// matching `SectionConfig::max_sessions` for user-defined sections, the
/// top-level `in_progress_limit` for the implicit "In Progress" catch-all,
/// or `None` when no limit is configured.
fn resolve_section_limit(
    name: &str,
    sections: &[crate::session::SectionConfig],
    in_progress_limit: Option<u32>,
) -> Option<u32> {
    if name == crate::session::IN_PROGRESS {
        return in_progress_limit;
    }
    sections
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| s.max_sessions)
}

pub(super) fn build_section_grouped_items(
    snapshot: &WorkspaceSnapshot,
    sections: &[crate::session::SectionConfig],
    in_progress_limit: Option<u32>,
    agent_states: &HashMap<SessionId, AgentState>,
    collapsed_sections: &std::collections::HashSet<String>,
) -> Vec<SessionListItem> {
    let by_id = session_index(snapshot);
    let groups = crate::session::build_sections(&snapshot.sessions, sections);

    let mut projects: Vec<&ProjectInfo> = snapshot.projects.iter().collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name));

    let mut items = Vec::new();
    for (group_idx, group) in groups.iter().enumerate() {
        if group_idx > 0 {
            items.push(SessionListItem::Spacer);
        }
        let collapsed = collapsed_sections.contains(&group.name);
        items.push(SessionListItem::SectionHeader {
            name: group.name.clone(),
            count: group.sessions.len(),
            collapsed,
            max_sessions: resolve_section_limit(&group.name, sections, in_progress_limit),
        });

        if collapsed {
            continue;
        }

        let is_in_progress = group.name == crate::session::IN_PROGRESS;
        // Preserve group sort order (oldest-first) while partitioning by project.
        let mut by_project: std::collections::HashMap<crate::session::ProjectId, Vec<SessionId>> =
            Default::default();
        let mut project_order: Vec<crate::session::ProjectId> = Vec::new();
        for sid in &group.sessions {
            if let Some(session) = by_id.get(sid) {
                by_project.entry(session.project_id).or_default().push(*sid);
                if !project_order.contains(&session.project_id) {
                    project_order.push(session.project_id);
                }
            }
        }

        for project in &projects {
            let project_sessions = by_project.get(&project.id);
            let count = project_sessions.map(|v| v.len()).unwrap_or(0);
            // In Progress shows every project (even empty ones); other
            // sections only show projects that have sessions in them.
            if !is_in_progress && count == 0 {
                continue;
            }
            items.push(SessionListItem::Project {
                id: project.id,
                name: project.name.clone(),
                repo_path: project.repo_path.clone(),
                main_branch: project.main_branch.clone(),
                worktree_count: count,
                nested: true,
            });
            if let Some(sids) = project_sessions {
                for sid in sids {
                    if let Some(session) = by_id.get(sid).copied() {
                        items.push(worktree_item(session, agent_states, None, false));
                    }
                }
            }
        }
    }
    items
}

/// Like `build_section_grouped_items` but treats each PR stack as one unit:
/// the whole stack lands in the section chosen by its newest leaf (with
/// `section_override` walked closest-to-leaf-first), and stack indentation is
/// preserved via `stacked_child: true` on non-root members.
pub(super) fn build_stacked_section_items(
    snapshot: &WorkspaceSnapshot,
    sections: &[crate::session::SectionConfig],
    in_progress_limit: Option<u32>,
    agent_states: &HashMap<SessionId, AgentState>,
    collapsed_sections: &std::collections::HashSet<String>,
) -> Vec<SessionListItem> {
    use chrono::{DateTime, Utc};

    #[derive(Clone)]
    struct GroupRender {
        sort_key: DateTime<Utc>,
        // Tiebreaker for groups whose leaves share an entered_section_at —
        // common when one apply_assignment pass stamps multiple sessions
        // with the same `now`. Without a stable tiebreaker, HashMap-
        // randomised insertion order leaks into the sort and the UI
        // appears to churn on every refresh.
        leaf_id: SessionId,
        order: Vec<(SessionId, bool)>,
    }

    let by_id = session_index(snapshot);

    // Stable project order. Ties on `name` (unusual but possible) fall
    // back to project id so we never depend on HashMap iteration order.
    let mut projects: Vec<&ProjectInfo> = snapshot.projects.iter().collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));

    // section_name → project_id → Vec<GroupRender>
    let mut by_section: std::collections::HashMap<
        String,
        std::collections::HashMap<crate::session::ProjectId, Vec<GroupRender>>,
    > = std::collections::HashMap::new();
    let valid_section =
        |name: &str| name == crate::session::IN_PROGRESS || sections.iter().any(|s| s.name == name);

    for project in &projects {
        // Sort by (created_at, id) so any downstream max_by_key on this
        // slice (e.g. fan-out children with identical created_at in
        // `stack_top`) picks a deterministic winner.
        let mut project_sessions: Vec<&SessionInfo> = snapshot
            .sessions
            .iter()
            .filter(|s| s.project_id == project.id)
            .collect();
        if project_sessions.is_empty() {
            continue;
        }
        project_sessions.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.session_id.cmp(&b.session_id))
        });

        // Bucket every session by its stack root (returns self for
        // unstacked). Track first-encounter root order so we iterate
        // groups deterministically below without leaking
        // HashMap-iteration order into the output.
        let mut groups: std::collections::HashMap<SessionId, Vec<&SessionInfo>> =
            std::collections::HashMap::new();
        let mut group_roots: Vec<SessionId> = Vec::new();
        for s in &project_sessions {
            let root_id = crate::session::stack_root(s.session_id, &project_sessions);
            if !groups.contains_key(&root_id) {
                group_roots.push(root_id);
            }
            groups.entry(root_id).or_default().push(s);
        }

        for root_id in group_roots {
            let members = groups.remove(&root_id).unwrap_or_default();
            // Pick the leaf in the whole subgraph; its section drives placement.
            let leaf_id = crate::session::stack_top(root_id, &project_sessions);
            let Some(leaf) = members.iter().find(|s| s.session_id == leaf_id).copied() else {
                continue;
            };

            // Walk leaf → root. First *valid* section_override encountered
            // wins (stale ones are skipped, see below); overrides on off-path
            // siblings are not considered.
            let mut effective: Option<String> = None;
            let mut cursor = leaf.session_id;
            for _ in 0..project_sessions.len() {
                let Some(cur) = project_sessions
                    .iter()
                    .find(|s| s.session_id == cursor)
                    .copied()
                else {
                    break;
                };
                // Only a *valid* override stops the walk. A stale override
                // (naming a section no longer in config) is ignored — same as
                // `assign_section` — so the session falls back to its
                // `current_section` instead of being dumped into In Progress.
                if let Some(ovr) = &cur.section_override
                    && valid_section(ovr)
                {
                    effective = Some(ovr.clone());
                    break;
                }
                match crate::session::resolve_stack_parent(cur, &project_sessions) {
                    Some(parent) => cursor = parent,
                    None => break,
                }
            }
            let section_name = effective
                .or_else(|| leaf.current_section.clone())
                .filter(|n| valid_section(n))
                .unwrap_or_else(|| crate::session::IN_PROGRESS.to_string());

            // Order within the group: stack-aware (root first, children
            // indented). build_session_order resolves parents only against
            // the slice it's given, so passing just the group's members
            // keeps the root flat and descendants indented even when the
            // subgraph fans out.
            let order = build_session_order(&members);

            by_section
                .entry(section_name)
                .or_default()
                .entry(project.id)
                .or_default()
                .push(GroupRender {
                    sort_key: leaf.entered_section_at.unwrap_or_default(),
                    leaf_id,
                    order,
                });
        }
    }

    // Emit IN_PROGRESS first, then user sections in declared order — same
    // overall layout the plain section view uses.
    let section_order: Vec<String> = std::iter::once(crate::session::IN_PROGRESS.to_string())
        .chain(sections.iter().map(|s| s.name.clone()))
        .collect();

    let mut items = Vec::new();
    for (idx, section_name) in section_order.iter().enumerate() {
        if idx > 0 {
            items.push(SessionListItem::Spacer);
        }

        let project_groups = by_section.get(section_name);
        let total_count: usize = project_groups
            .map(|m| {
                m.values()
                    .flat_map(|g| g.iter())
                    .map(|g| g.order.len())
                    .sum()
            })
            .unwrap_or(0);

        let collapsed = collapsed_sections.contains(section_name);
        items.push(SessionListItem::SectionHeader {
            name: section_name.clone(),
            count: total_count,
            collapsed,
            max_sessions: resolve_section_limit(section_name, sections, in_progress_limit),
        });
        if collapsed {
            continue;
        }

        let is_in_progress = section_name == crate::session::IN_PROGRESS;
        for project in &projects {
            let mut groups_in_proj = project_groups
                .and_then(|m| m.get(&project.id))
                .cloned()
                .unwrap_or_default();
            let project_count: usize = groups_in_proj.iter().map(|g| g.order.len()).sum();
            if !is_in_progress && project_count == 0 {
                continue;
            }
            items.push(SessionListItem::Project {
                id: project.id,
                name: project.name.clone(),
                repo_path: project.repo_path.clone(),
                main_branch: project.main_branch.clone(),
                worktree_count: project_count,
                nested: true,
            });

            // Stacks within a section sort by their leaf's
            // entered_section_at, with leaf_id as a stable tiebreaker so
            // batched apply_assignment calls (which stamp many sessions
            // with the same `now`) don't make the view churn.
            groups_in_proj
                .sort_by(|a, b| a.sort_key.cmp(&b.sort_key).then(a.leaf_id.cmp(&b.leaf_id)));
            for group in groups_in_proj {
                for (sid, stacked_child) in group.order {
                    if let Some(session) = by_id.get(&sid).copied() {
                        items.push(worktree_item(session, agent_states, None, stacked_child));
                    }
                }
            }
        }
    }
    items
}

/// Apply freshly-detected agent states for the sessions just viewed during an
/// attach, leaving every other session's entry untouched.
///
/// Returning from an attach must not blank the whole tree (which happens if the
/// agent-state map is cleared wholesale) nor drop genuine background
/// `Working → Idle` notifications. Only the sessions the user actually saw get
/// their state overwritten here; because the user was watching them, the
/// refreshed state is applied directly without running unread detection, so
/// their own transitions are never re-flagged as unread. Every other session
/// keeps its prior state, preserving the baseline a later poll diffs against.
pub(super) fn apply_viewed_session_refresh(
    agent_states: &mut HashMap<SessionId, AgentState>,
    refreshed: HashMap<SessionId, AgentState>,
) {
    agent_states.extend(refreshed);
}

/// Preview the stack-retarget that deleting `session_id` would trigger, derived
/// from a workspace snapshot: `(number of direct stacked children, branch they'd
/// be retargeted onto)`. Returns `None` when the session has no direct stacked
/// children, so the delete confirmation only mentions retargeting when it
/// actually applies.
///
/// DTO twin of [`AppState::stack_retarget_preview`](crate::config::storage::AppState::stack_retarget_preview):
/// the delete-confirm dialog derives its preview from the cached snapshot rather
/// than reading the store, so a remote backend's snapshot drives it identically.
pub(super) fn stack_retarget_preview_from_snapshot(
    snapshot: &WorkspaceSnapshot,
    session_id: SessionId,
) -> Option<(usize, String)> {
    let deleted = snapshot
        .sessions
        .iter()
        .find(|s| s.session_id == session_id)?;
    let project_id = deleted.project_id;
    let main_branch = snapshot
        .projects
        .iter()
        .find(|p| p.id == project_id)?
        .main_branch
        .clone();
    let project_sessions: Vec<&SessionInfo> = snapshot
        .sessions
        .iter()
        .filter(|s| s.project_id == project_id)
        .collect();

    let child_ids: Vec<SessionId> = project_sessions
        .iter()
        .filter(|s| {
            crate::session::resolve_stack_parent(**s, &project_sessions) == Some(session_id)
        })
        .map(|s| s.session_id)
        .collect();
    if child_ids.is_empty() {
        return None;
    }

    let new_base_branch = crate::session::resolve_stack_parent(deleted, &project_sessions)
        .and_then(|pid| project_sessions.iter().find(|s| s.session_id == pid))
        .map(|p| p.branch.clone())
        .unwrap_or(main_branch);
    Some((child_ids.len(), new_base_branch))
}

#[cfg(test)]
mod unread_transition_tests {
    use super::*;
    use crate::api::detect_unread_transitions;
    use crate::session::SessionId;
    use std::collections::HashMap;

    #[test]
    fn viewed_refresh_preserves_background_unread() {
        // Scenario: attached to session A while a background session B is also
        // running. Both finish (Working → Idle) during the attach. On detach we
        // refresh only the viewed session (A). A subsequent poll must still flag
        // B as unread (we never saw it finish) while leaving A alone.
        let a = SessionId::new();
        let b = SessionId::new();

        // Pre-attach baseline: both working.
        let mut agent_states = HashMap::from([(a, AgentState::Working), (b, AgentState::Working)]);

        // Detach refreshes only the viewed session, now observed idle.
        apply_viewed_session_refresh(&mut agent_states, HashMap::from([(a, AgentState::Idle)]));

        // A reflects its observed state; B's baseline is untouched (not wiped).
        assert_eq!(agent_states.get(&a), Some(&AgentState::Idle));
        assert_eq!(agent_states.get(&b), Some(&AgentState::Working));

        // Next background poll reports both idle.
        let poll = HashMap::from([(a, AgentState::Idle), (b, AgentState::Idle)]);
        let unread = detect_unread_transitions(&agent_states, &poll);

        // Only B is flagged: A's finish was watched, B's was not. A wholesale
        // clear() on detach would have dropped B's notification entirely.
        assert_eq!(unread, vec![b]);
    }
}

#[cfg(test)]
mod stack_order_tests {
    use super::*;
    use crate::session::{ProjectId, WorktreeSession};
    use chrono::{Duration as ChronoDuration, Utc};
    use std::path::PathBuf;

    fn make_session(title: &str, branch: &str, created_offset_secs: i64) -> WorktreeSession {
        let mut s = WorktreeSession::new(
            ProjectId::new(),
            title,
            branch,
            PathBuf::from("/tmp/wt"),
            "claude",
        );
        s.created_at = Utc::now() + ChronoDuration::seconds(created_offset_secs);
        s
    }

    #[test]
    fn ordering_unstacked_only_sorts_newest_first() {
        let a = make_session("a", "a", 0);
        let b = make_session("b", "b", 10);
        let c = make_session("c", "c", 20);
        let order = build_session_order(&[&a, &b, &c]);
        assert_eq!(
            order,
            vec![(c.id, false), (b.id, false), (a.id, false)],
            "newer sessions should appear first at the root level"
        );
    }

    #[test]
    fn ordering_single_stack_emits_base_then_children_in_stack_order() {
        // base (oldest) ← child1 ← child2; all stacked, base at root indent.
        let base = make_session("base", "base-br", 0);
        let mut child1 = make_session("c1", "c1-br", 5);
        child1.stack_parent_session_id = Some(base.id);
        let mut child2 = make_session("c2", "c2-br", 10);
        child2.stack_parent_session_id = Some(child1.id);

        let order = build_session_order(&[&base, &child1, &child2]);
        assert_eq!(
            order,
            vec![(base.id, false), (child1.id, true), (child2.id, true)]
        );
    }

    #[test]
    fn ordering_two_independent_stacks_interleave_by_base_created_at() {
        // Two stacks; their bases sort by created_at among root rows. Each
        // stack's children appear directly beneath its base.
        let base_a = make_session("base-a", "base-a", 0);
        let mut child_a = make_session("child-a", "child-a", 1);
        child_a.stack_parent_session_id = Some(base_a.id);

        let base_b = make_session("base-b", "base-b", 20);
        let mut child_b = make_session("child-b", "child-b", 21);
        child_b.stack_parent_session_id = Some(base_b.id);

        let order = build_session_order(&[&base_a, &child_a, &base_b, &child_b]);
        assert_eq!(
            order,
            vec![
                (base_b.id, false), // newer base first at root level
                (child_b.id, true),
                (base_a.id, false),
                (child_a.id, true),
            ]
        );
    }

    #[test]
    fn ordering_mixed_stack_and_unstacked_interleaves_correctly() {
        let base = make_session("base", "base", 0);
        let mut child = make_session("child", "child", 5);
        child.stack_parent_session_id = Some(base.id);
        let solo = make_session("solo", "solo", 10);
        let order = build_session_order(&[&base, &child, &solo]);
        assert_eq!(
            order,
            vec![
                (solo.id, false), // newest root first
                (base.id, false),
                (child.id, true), // follows its base
            ]
        );
    }

    #[test]
    fn ordering_orphan_stack_parent_is_treated_as_root() {
        let mut orphan = make_session("orphan", "orphan", 0);
        orphan.stack_parent_session_id = Some(SessionId::new()); // dangling
        let order = build_session_order(&[&orphan]);
        assert_eq!(order, vec![(orphan.id, false)]);
    }

    #[test]
    fn ordering_sibling_children_of_same_base_both_indent() {
        // Two sessions sharing one base — both should render as stacked
        // children, in created_at order, both indented.
        let base = make_session("base", "base", 0);
        let mut c1 = make_session("c1", "c1", 5);
        c1.stack_parent_session_id = Some(base.id);
        let mut c2 = make_session("c2", "c2", 10);
        c2.stack_parent_session_id = Some(base.id);
        let order = build_session_order(&[&base, &c1, &c2]);
        assert_eq!(order, vec![(base.id, false), (c1.id, true), (c2.id, true)]);
    }

    #[test]
    fn ordering_pr_base_matching_session_forms_stack() {
        // No local link — GitHub PR info alone should form the stack.
        let base = make_session("base", "base-br", 0);
        let mut child = make_session("child", "child-br", 5);
        child.pr_base_branch = Some("base-br".to_string());
        let order = build_session_order(&[&base, &child]);
        assert_eq!(order, vec![(base.id, false), (child.id, true)]);
    }

    fn make_session_in_section(
        title: &str,
        branch: &str,
        created_offset_secs: i64,
        current_section: &str,
    ) -> WorktreeSession {
        let mut s = make_session(title, branch, created_offset_secs);
        s.current_section = Some(current_section.to_string());
        // Stamp section-entry time to mirror the created offset so the leaf's
        // entered_section_at uniquely identifies the group's sort position.
        s.entered_section_at = Utc::now() + ChronoDuration::seconds(created_offset_secs);
        s
    }

    fn section_named(name: &str) -> crate::session::SectionConfig {
        crate::session::SectionConfig {
            name: name.to_string(),
            ..Default::default()
        }
    }

    /// Build the DTO [`WorkspaceSnapshot`] the tree builders now consume, from a
    /// list of domain sessions — same shaped input as before, projected through
    /// the production `workspace_snapshot_from_state` so tests exercise the real
    /// conversion path.
    fn appstate_from(sessions: Vec<WorktreeSession>) -> WorkspaceSnapshot {
        let mut state = crate::config::AppState::default();
        // Group sessions by their project_id so projects with multiple
        // worktrees stay linked correctly.
        let mut project_titles: std::collections::HashMap<ProjectId, String> = Default::default();
        for s in &sessions {
            project_titles
                .entry(s.project_id)
                .or_insert_with(|| format!("p-{}", &s.project_id.to_string()));
        }
        for (pid, name) in project_titles {
            let mut project = crate::session::Project::new(&name, PathBuf::from("/tmp"), "main");
            project.id = pid;
            state.projects.insert(pid, project);
        }
        for s in sessions {
            let pid = s.project_id;
            state.projects.get_mut(&pid).unwrap().add_worktree(s.id);
            state.sessions.insert(s.id, s);
        }
        workspace_snapshot_from_state(&state)
    }

    #[test]
    fn retarget_preview_reports_children_and_new_base() {
        let pid = ProjectId::new();
        let base = {
            let mut s = make_session("base", "base-br", 0);
            s.project_id = pid;
            s
        };
        let child = {
            let mut s = make_session("child", "child-br", 5);
            s.project_id = pid;
            s.stack_parent_session_id = Some(base.id);
            s
        };
        let base_id = base.id;
        let child_id = child.id;
        let snapshot = appstate_from(vec![base, child]);

        // Deleting the stack base retargets its one child onto the project's
        // main branch (the base was the stack root).
        assert_eq!(
            stack_retarget_preview_from_snapshot(&snapshot, base_id),
            Some((1, "main".to_string()))
        );
        // The leaf child has no stacked children → no retarget preview.
        assert_eq!(
            stack_retarget_preview_from_snapshot(&snapshot, child_id),
            None
        );
    }

    #[test]
    fn stacked_sections_group_whole_stack_under_leaf_section() {
        // base.current_section = "Review" (older), child.current_section = "Open" (leaf).
        // In the new stacked-section view the whole stack should appear under
        // "Open" (the leaf's section), with `child` indented beneath `base`.
        let project_id = ProjectId::new();
        let mut base = make_session_in_section("base", "base", 0, "Review");
        base.project_id = project_id;
        let mut child = make_session_in_section("child", "child", 10, "Open");
        child.project_id = project_id;
        child.stack_parent_session_id = Some(base.id);

        let state = appstate_from(vec![base.clone(), child.clone()]);
        let sections = vec![section_named("Open"), section_named("Review")];
        let agent_states = HashMap::new();
        let collapsed = std::collections::HashSet::new();

        let items = build_stacked_section_items(&state, &sections, None, &agent_states, &collapsed);

        // Walk items: find the "Open" header, then base+child should follow.
        let found_open = items.iter().any(
            |item| matches!(item, SessionListItem::SectionHeader { name, .. } if name == "Open"),
        );
        assert!(
            found_open,
            "Open section header should be present: {items:?}"
        );

        // After the Open header, expect: Project → base (stacked_child:false) → child (stacked_child:true)
        let after = items
            .iter()
            .skip_while(
                |i| !matches!(i, SessionListItem::SectionHeader { name, .. } if name == "Open"),
            )
            .skip(1)
            .collect::<Vec<_>>();
        let session_rows: Vec<_> = after
            .iter()
            .filter_map(|i| match i {
                SessionListItem::Worktree {
                    id, stacked_child, ..
                } => Some((*id, *stacked_child)),
                _ => None,
            })
            .take_while(|_| true)
            .collect();
        assert_eq!(
            session_rows,
            vec![(base.id, false), (child.id, true)],
            "stack should render under leaf's section with indentation preserved"
        );

        // And there should be no Worktree row under "Review".
        let review_rows: Vec<_> = items
            .iter()
            .skip_while(
                |i| !matches!(i, SessionListItem::SectionHeader { name, .. } if name == "Review"),
            )
            .skip(1)
            .take_while(|i| {
                !matches!(
                    i,
                    SessionListItem::SectionHeader { .. } | SessionListItem::Spacer
                )
            })
            .filter(|i| matches!(i, SessionListItem::Worktree { .. }))
            .collect();
        assert!(
            review_rows.is_empty(),
            "stack-base should have moved out of Review section: {review_rows:?}"
        );
    }

    #[test]
    fn stacked_sections_override_closest_to_leaf_wins() {
        // base has section_override = "Pinned"; child (leaf) has no override.
        // Whole stack lands in "Pinned".
        let project_id = ProjectId::new();
        let mut base = make_session_in_section("base", "base", 0, "Review");
        base.project_id = project_id;
        base.section_override = Some("Pinned".to_string());
        let mut child = make_session_in_section("child", "child", 10, "Open");
        child.project_id = project_id;
        child.stack_parent_session_id = Some(base.id);

        let state = appstate_from(vec![base.clone(), child.clone()]);
        let sections = vec![
            section_named("Open"),
            section_named("Review"),
            section_named("Pinned"),
        ];
        let items = build_stacked_section_items(
            &state,
            &sections,
            None,
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let pinned_rows: Vec<_> = items
            .iter()
            .skip_while(
                |i| !matches!(i, SessionListItem::SectionHeader { name, .. } if name == "Pinned"),
            )
            .skip(1)
            .take_while(|i| {
                !matches!(
                    i,
                    SessionListItem::SectionHeader { .. } | SessionListItem::Spacer
                )
            })
            .filter_map(|i| match i {
                SessionListItem::Worktree {
                    id, stacked_child, ..
                } => Some((*id, *stacked_child)),
                _ => None,
            })
            .collect();
        assert_eq!(
            pinned_rows,
            vec![(base.id, false), (child.id, true)],
            "whole stack should land in the closest-to-leaf overridden section"
        );
    }

    #[test]
    fn stacked_sections_stale_override_falls_back_to_current_section() {
        // A session pinned (section_override) to a section that no longer
        // exists in config must fall back to its valid current_section, not
        // be dumped into In Progress.
        let project_id = ProjectId::new();
        let mut s = make_session_in_section("s", "s", 0, "Open");
        s.project_id = project_id;
        s.section_override = Some("Deleted Section".to_string());

        let state = appstate_from(vec![s.clone()]);
        let sections = vec![section_named("Open"), section_named("Review")];
        let items = build_stacked_section_items(
            &state,
            &sections,
            None,
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );

        // Walk items tracking the enclosing section header, then read off
        // which header the session row landed under.
        let mut current_header: Option<String> = None;
        let mut landed: Option<String> = None;
        for item in &items {
            match item {
                SessionListItem::SectionHeader { name, .. } => {
                    current_header = Some(name.clone());
                }
                SessionListItem::Worktree { id, .. } if *id == s.id => {
                    landed = current_header.clone();
                }
                _ => {}
            }
        }
        assert_eq!(
            landed.as_deref(),
            Some("Open"),
            "stale override should defer to current_section, not In Progress"
        );
    }

    #[test]
    fn stacked_sections_fan_out_groups_share_root_and_use_newest_leaf() {
        // Fan-out: base has two children, B (older) and C (newer).
        // C is the newest leaf in the subgraph, so the whole stack (base+B+C)
        // appears under C's current_section.
        let project_id = ProjectId::new();
        let mut base = make_session_in_section("base", "base", 0, "Review");
        base.project_id = project_id;
        let mut b = make_session_in_section("b", "b", 5, "Review");
        b.project_id = project_id;
        b.stack_parent_session_id = Some(base.id);
        let mut c = make_session_in_section("c", "c", 20, "Open");
        c.project_id = project_id;
        c.stack_parent_session_id = Some(base.id);

        let state = appstate_from(vec![base.clone(), b.clone(), c.clone()]);
        let sections = vec![section_named("Open"), section_named("Review")];
        let items = build_stacked_section_items(
            &state,
            &sections,
            None,
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let open_session_ids: Vec<_> = items
            .iter()
            .skip_while(
                |i| !matches!(i, SessionListItem::SectionHeader { name, .. } if name == "Open"),
            )
            .skip(1)
            .take_while(|i| {
                !matches!(
                    i,
                    SessionListItem::SectionHeader { .. } | SessionListItem::Spacer
                )
            })
            .filter_map(|i| match i {
                SessionListItem::Worktree { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert!(
            open_session_ids.contains(&base.id)
                && open_session_ids.contains(&b.id)
                && open_session_ids.contains(&c.id),
            "all three subgraph members should appear under Open: {open_session_ids:?}"
        );

        let review_session_count = items
            .iter()
            .skip_while(
                |i| !matches!(i, SessionListItem::SectionHeader { name, .. } if name == "Review"),
            )
            .skip(1)
            .take_while(|i| {
                !matches!(
                    i,
                    SessionListItem::SectionHeader { .. } | SessionListItem::Spacer
                )
            })
            .filter(|i| matches!(i, SessionListItem::Worktree { .. }))
            .count();
        assert_eq!(
            review_session_count, 0,
            "Review should be empty because B follows the stack into Open"
        );
    }

    #[test]
    fn stacked_sections_sibling_override_off_leaf_path_is_ignored() {
        // base ← B (override "Pinned"), base ← C (newer leaf, no override).
        // The chosen leaf is C; walking C → base, B is off-path so its
        // override doesn't count. Stack goes to C's current_section.
        let project_id = ProjectId::new();
        let mut base = make_session_in_section("base", "base", 0, "Review");
        base.project_id = project_id;
        let mut b = make_session_in_section("b", "b", 5, "Review");
        b.project_id = project_id;
        b.stack_parent_session_id = Some(base.id);
        b.section_override = Some("Pinned".to_string());
        let mut c = make_session_in_section("c", "c", 20, "Open");
        c.project_id = project_id;
        c.stack_parent_session_id = Some(base.id);

        let state = appstate_from(vec![base.clone(), b.clone(), c.clone()]);
        let sections = vec![
            section_named("Open"),
            section_named("Review"),
            section_named("Pinned"),
        ];
        let items = build_stacked_section_items(
            &state,
            &sections,
            None,
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let pinned_count = items
            .iter()
            .skip_while(
                |i| !matches!(i, SessionListItem::SectionHeader { name, .. } if name == "Pinned"),
            )
            .skip(1)
            .take_while(|i| {
                !matches!(
                    i,
                    SessionListItem::SectionHeader { .. } | SessionListItem::Spacer
                )
            })
            .filter(|i| matches!(i, SessionListItem::Worktree { .. }))
            .count();
        assert_eq!(
            pinned_count, 0,
            "off-leaf-path overrides should not pull the stack into their section"
        );

        let open_count = items
            .iter()
            .skip_while(
                |i| !matches!(i, SessionListItem::SectionHeader { name, .. } if name == "Open"),
            )
            .skip(1)
            .take_while(|i| {
                !matches!(
                    i,
                    SessionListItem::SectionHeader { .. } | SessionListItem::Spacer
                )
            })
            .filter(|i| matches!(i, SessionListItem::Worktree { .. }))
            .count();
        assert_eq!(
            open_count, 3,
            "all three subgraph members should appear under Open: {items:?}"
        );
    }

    #[test]
    fn stacked_sections_output_stable_across_repeated_calls_with_tied_timestamps() {
        // Two stacks whose leaves both entered the section at the same
        // instant (apply_assignment uses one `now` for the whole batch).
        // Output must be deterministic between calls or the UI churns on
        // every refresh.
        let project_id = ProjectId::new();
        let same_ts = Utc::now();
        let mk = |title: &str, branch: &str| {
            let mut s = make_session_in_section(title, branch, 0, "Open");
            s.project_id = project_id;
            s.entered_section_at = same_ts;
            s
        };
        let mut base_a = mk("base-a", "base-a");
        base_a.created_at = same_ts;
        let mut child_a = mk("child-a", "child-a");
        child_a.created_at = same_ts;
        child_a.stack_parent_session_id = Some(base_a.id);
        let mut base_b = mk("base-b", "base-b");
        base_b.created_at = same_ts;
        let mut child_b = mk("child-b", "child-b");
        child_b.created_at = same_ts;
        child_b.stack_parent_session_id = Some(base_b.id);

        let state = appstate_from(vec![
            base_a.clone(),
            child_a.clone(),
            base_b.clone(),
            child_b.clone(),
        ]);
        let sections = vec![section_named("Open")];
        let collapsed = std::collections::HashSet::new();

        let first =
            build_stacked_section_items(&state, &sections, None, &HashMap::new(), &collapsed);
        // Call many times — every iteration constructs fresh internal
        // HashMaps with a new RandomState, so any non-determinism shows up
        // here. 32 calls is well past the birthday-paradox threshold.
        for _ in 0..32 {
            let again =
                build_stacked_section_items(&state, &sections, None, &HashMap::new(), &collapsed);
            assert_eq!(
                again, first,
                "build_stacked_section_items must produce identical output on every call"
            );
        }
    }

    #[test]
    fn stacked_sections_sort_groups_by_leaf_entered_section_at() {
        // Two unstacked sessions both in "Open"; the newer one's
        // entered_section_at should sort it... wait — build_sections sorts
        // oldest-first within a section. The newer leaf has a later
        // entered_section_at and therefore appears AFTER the older one.
        let project_id = ProjectId::new();
        let mut older = make_session_in_section("older", "older", 0, "Open");
        older.project_id = project_id;
        let mut newer = make_session_in_section("newer", "newer", 20, "Open");
        newer.project_id = project_id;

        let state = appstate_from(vec![older.clone(), newer.clone()]);
        let sections = vec![section_named("Open")];
        let items = build_stacked_section_items(
            &state,
            &sections,
            None,
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let open_session_order: Vec<_> = items
            .iter()
            .skip_while(
                |i| !matches!(i, SessionListItem::SectionHeader { name, .. } if name == "Open"),
            )
            .skip(1)
            .take_while(|i| {
                !matches!(
                    i,
                    SessionListItem::SectionHeader { .. } | SessionListItem::Spacer
                )
            })
            .filter_map(|i| match i {
                SessionListItem::Worktree { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(
            open_session_order,
            vec![older.id, newer.id],
            "Open section should be sorted by entered_section_at (oldest first)"
        );
    }

    #[test]
    fn resolve_section_limit_uses_in_progress_limit_for_catch_all() {
        let sections = vec![section_named("Open")];
        assert_eq!(
            super::resolve_section_limit(crate::session::IN_PROGRESS, &sections, Some(3)),
            Some(3)
        );
        assert_eq!(
            super::resolve_section_limit(crate::session::IN_PROGRESS, &sections, None),
            None
        );
    }

    #[test]
    fn resolve_section_limit_reads_max_sessions_from_matching_config() {
        let sections = vec![
            section_named("Open"),
            crate::session::SectionConfig {
                name: "Review".into(),
                max_sessions: Some(2),
                ..Default::default()
            },
        ];
        assert_eq!(
            super::resolve_section_limit("Review", &sections, None),
            Some(2)
        );
        assert_eq!(super::resolve_section_limit("Open", &sections, None), None);
        assert_eq!(
            super::resolve_section_limit("Missing", &sections, Some(99)),
            None,
            "in_progress_limit must not leak into other section names"
        );
    }

    #[test]
    fn section_grouped_header_carries_max_sessions_from_config() {
        let project_id = ProjectId::new();
        let mut s = make_session_in_section("s", "s", 0, "Review");
        s.project_id = project_id;

        let state = appstate_from(vec![s]);
        let sections = vec![crate::session::SectionConfig {
            name: "Review".into(),
            max_sessions: Some(5),
            ..Default::default()
        }];

        let items = super::build_section_grouped_items(
            &state,
            &sections,
            None,
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let review_header = items.iter().find_map(|i| match i {
            SessionListItem::SectionHeader {
                name,
                max_sessions,
                count,
                ..
            } if name == "Review" => Some((*count, *max_sessions)),
            _ => None,
        });
        assert_eq!(review_header, Some((1, Some(5))));
    }

    #[test]
    fn section_grouped_in_progress_header_carries_in_progress_limit() {
        let project_id = ProjectId::new();
        let mut s = make_session("a", "a", 0);
        s.project_id = project_id;

        let state = appstate_from(vec![s]);
        let sections: Vec<crate::session::SectionConfig> = vec![];

        let items = super::build_section_grouped_items(
            &state,
            &sections,
            Some(2),
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let ip_header = items.iter().find_map(|i| match i {
            SessionListItem::SectionHeader {
                name, max_sessions, ..
            } if name == crate::session::IN_PROGRESS => Some(*max_sessions),
            _ => None,
        });
        assert_eq!(ip_header, Some(Some(2)));
    }

    #[test]
    fn stacked_section_header_carries_max_sessions() {
        let project_id = ProjectId::new();
        let mut s = make_session_in_section("s", "s", 0, "Review");
        s.project_id = project_id;

        let state = appstate_from(vec![s]);
        let sections = vec![crate::session::SectionConfig {
            name: "Review".into(),
            max_sessions: Some(4),
            ..Default::default()
        }];

        let items = build_stacked_section_items(
            &state,
            &sections,
            Some(1),
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );

        let review_limit = items.iter().find_map(|i| match i {
            SessionListItem::SectionHeader {
                name, max_sessions, ..
            } if name == "Review" => Some(*max_sessions),
            _ => None,
        });
        let ip_limit = items.iter().find_map(|i| match i {
            SessionListItem::SectionHeader {
                name, max_sessions, ..
            } if name == crate::session::IN_PROGRESS => Some(*max_sessions),
            _ => None,
        });
        assert_eq!(review_limit, Some(Some(4)));
        assert_eq!(ip_limit, Some(Some(1)));
    }

    #[test]
    fn ordering_pr_base_matching_main_pops_child_to_root() {
        // When the PR retargets main (e.g. after the prior stack member was
        // merged), the child becomes a stack root — both base and ex-child are
        // root-level siblings.
        let base = make_session("base", "base-br", 0);
        let mut child = make_session("child", "child-br", 5);
        child.pr_base_branch = Some("main".to_string());
        // Local link still hanging around — PR data wins.
        child.stack_parent_session_id = Some(base.id);
        let order = build_session_order(&[&base, &child]);
        assert_eq!(
            order,
            vec![(child.id, false), (base.id, false)],
            "child with PR targeting main should pop to the root list"
        );
    }

    // -----------------------------------------------------------------------
    // Perf baseline: tree building over a large seeded state.
    //
    // The tree builders (`build_project_grouped_items` /
    // `build_section_grouped_items` / `build_stacked_section_items`) currently
    // read the local `AppState` directly. The Phase C refactor moves them onto
    // DTO snapshots behind a backend trait; this seeder + `#[ignore]`d timing
    // test pin the current cost so the refactor can prove it did not regress
    // tree building for local users.
    //
    // `seed_large_state` is intentionally reusable (`pub(super)`) so Phase C can
    // feed the *same* shaped state into the DTO-based builders and compare
    // apples-to-apples. Run with:
    //   cargo test -p claude-commander-core tree_build_perf_baseline -- --ignored --nocapture
    // -----------------------------------------------------------------------

    /// Build a deterministic `AppState` with `n_projects` projects, each holding
    /// `sessions_per_project` sessions. Roughly one in five sessions is a
    /// stacked child of the session before it (exercising `resolve_stack_parent`
    /// and the stack-ordering path), and sessions are round-robin assigned a
    /// `current_section` so the section builders have populated buckets. Also
    /// returns matching `agent_states` (one entry per session, cycling through
    /// the agent states) and a `sections` config for the section views.
    ///
    /// All timestamps are derived from a fixed base + per-session offset so the
    /// output ordering is fully determined (no wall-clock reads leak in).
    #[allow(clippy::type_complexity)]
    pub(super) fn seed_large_state(
        n_projects: usize,
        sessions_per_project: usize,
    ) -> (
        crate::config::AppState,
        HashMap<SessionId, AgentState>,
        Vec<crate::session::SectionConfig>,
    ) {
        use chrono::TimeZone;
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let section_names = ["Review", "Ready", "Blocked"];
        let sections: Vec<crate::session::SectionConfig> =
            section_names.iter().map(|n| section_named(n)).collect();
        let agent_cycle = [
            AgentState::Working,
            AgentState::Idle,
            AgentState::WaitingForInput,
            AgentState::Unknown,
        ];

        let mut state = crate::config::AppState::default();
        let mut agent_states = HashMap::new();
        let mut global_idx: i64 = 0;

        for p in 0..n_projects {
            let project = crate::session::Project::new(
                format!("project-{p:02}"),
                PathBuf::from("/tmp/repo"),
                "main",
            );
            let project_id = project.id;
            state.projects.insert(project_id, project);

            let mut prev_in_project: Option<SessionId> = None;
            for s in 0..sessions_per_project {
                let mut session = WorktreeSession::new(
                    project_id,
                    format!("session-{p:02}-{s:03}"),
                    format!("branch-{p:02}-{s:03}"),
                    PathBuf::from("/tmp/wt"),
                    if s % 3 == 0 { "codex" } else { "claude" },
                );
                let ts = base + ChronoDuration::seconds(global_idx);
                session.created_at = ts;
                session.entered_section_at = ts;
                // Every 5th session (that has a predecessor) stacks on the one
                // before it, forming short chains within the project.
                if s % 5 == 0 && s != 0 {
                    session.stack_parent_session_id = prev_in_project;
                }
                // Spread sessions across the catch-all + configured sections.
                session.current_section = match s % 4 {
                    0 => None, // In Progress catch-all
                    other => Some(section_names[other - 1].to_string()),
                };
                session.unread = s % 7 == 0;
                if s % 6 == 0 {
                    session.pr_number = Some(1000 + global_idx as u32);
                    session.pr_state = Some(crate::git::PrState::Open);
                }

                agent_states.insert(session.id, agent_cycle[(global_idx as usize) % 4]);
                prev_in_project = Some(session.id);

                let sid = session.id;
                state.sessions.insert(sid, session);
                state
                    .projects
                    .get_mut(&project_id)
                    .unwrap()
                    .add_worktree(sid);
                global_idx += 1;
            }
        }
        (state, agent_states, sections)
    }

    #[test]
    #[ignore = "perf baseline; run explicitly with --ignored --nocapture"]
    fn tree_build_perf_baseline() {
        // ~100 sessions across ~10 projects, matching the Phase C brief.
        let (state, agent_states, sections) = seed_large_state(10, 10);
        let session_count = state.sessions.len();
        assert!(
            session_count >= 100,
            "expected at least 100 sessions, got {session_count}"
        );
        let collapsed = std::collections::HashSet::new();
        // Project into the DTO snapshot the builders now consume, once, outside
        // the timed loop — we measure the builders, not snapshot construction
        // (the cached snapshot is built on change, not per refresh).
        let snapshot = workspace_snapshot_from_state(&state);

        // Warm up so the first-touch allocation cost doesn't dominate the timing.
        for _ in 0..50 {
            std::hint::black_box(build_project_grouped_items(&snapshot, &agent_states));
        }

        let iterations = 2_000u32;
        let time = |label: &str, f: &mut dyn FnMut()| {
            let start = std::time::Instant::now();
            for _ in 0..iterations {
                f();
            }
            let per = start.elapsed() / iterations;
            println!("{label:<28} {per:?}/build ({session_count} sessions)");
            // Generous ceiling: a build of ~100 sessions is microsecond-scale;
            // 5ms leaves multiple orders of magnitude of slack while still
            // catching a catastrophic regression (e.g. an accidental O(n^2)).
            assert!(
                per < std::time::Duration::from_millis(5),
                "{label} regressed: {per:?}/build exceeds the 5ms ceiling"
            );
        };

        time("project_grouped", &mut || {
            std::hint::black_box(build_project_grouped_items(&snapshot, &agent_states));
        });
        time("section_grouped", &mut || {
            std::hint::black_box(build_section_grouped_items(
                &snapshot,
                &sections,
                Some(5),
                &agent_states,
                &collapsed,
            ));
        });
        time("section_stacks", &mut || {
            std::hint::black_box(build_stacked_section_items(
                &snapshot,
                &sections,
                Some(5),
                &agent_states,
                &collapsed,
            ));
        });
    }
}
