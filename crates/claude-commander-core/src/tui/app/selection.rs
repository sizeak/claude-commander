//! Selection and navigation: update selection, scroll dispatch, session number jumping.

use super::*;

/// Maximum delay between two same-row left clicks for them to count as a
/// double-click that triggers `UserCommand::Select`.
pub(super) const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// The identifying bits copied out of the selected list row, so the item's
/// immutable borrow of `list_items` ends before we resolve the owning backend
/// and mutate the selection fields.
enum SelKind {
    Project(ProjectId),
    Worktree(SessionId, ProjectId),
    /// A header row: clears both selections.
    Clear,
    /// A spacer: leaves the selection untouched.
    Keep,
}

impl App {
    /// The backend that owns session `id`, by scanning cached snapshots. Session
    /// ids are globally unique (UUIDs), so at most one backend matches; defaults
    /// to the local backend when none does (e.g. a stale ref).
    pub(super) fn backend_of_session(&self, id: SessionId) -> BackendId {
        self.backends
            .iter()
            .find(|h| h.view.snapshot.sessions.iter().any(|s| s.session_id == id))
            .map(|h| h.id)
            .unwrap_or(LOCAL_BACKEND_ID)
    }

    /// The backend that owns project `id`, by scanning cached snapshots.
    /// Defaults to the local backend when none matches.
    pub(super) fn backend_of_project(&self, id: ProjectId) -> BackendId {
        self.backends
            .iter()
            .find(|h| h.view.snapshot.projects.iter().any(|p| p.id == id))
            .map(|h| h.id)
            .unwrap_or(LOCAL_BACKEND_ID)
    }

    /// Whether backend `id`'s cached connection is `Connected`. A missing
    /// backend (stale id) counts as connected so callers don't wrongly gate.
    fn backend_is_connected(&self, id: BackendId) -> bool {
        self.backend(id)
            .map(|h| {
                matches!(
                    h.view.connection,
                    crate::backend::ConnectionState::Connected
                )
            })
            .unwrap_or(true)
    }

    /// Capabilities of backend `id`. A missing backend (stale id) falls back to
    /// the local (all-on) set so callers don't wrongly gate.
    fn backend_capabilities(&self, id: BackendId) -> crate::backend::BackendCapabilities {
        self.backend(id)
            .map(|h| h.backend.capabilities())
            .unwrap_or(crate::backend::BackendCapabilities::LOCAL)
    }

    /// Update selection tracking based on list position
    pub(super) fn update_selection(&mut self) {
        let old_session = self.ui_state.selected_session_id;
        let was_on_project = old_session.is_none() && self.ui_state.selected_project_id.is_some();

        let selected =
            self.ui_state
                .list_state
                .selected()
                .and_then(|idx| self.ui_state.list_items.get(idx))
                .map(|item| match item {
                    SessionListItem::Project { id, .. } => SelKind::Project(*id),
                    SessionListItem::Worktree { id, project_id, .. } => {
                        SelKind::Worktree(*id, *project_id)
                    }
                    SessionListItem::SectionHeader { .. }
                    | SessionListItem::ServerHeader { .. } => SelKind::Clear,
                    SessionListItem::Spacer => SelKind::Keep,
                });

        match selected {
            Some(SelKind::Project(id)) => {
                let backend = self.backend_of_project(id);
                self.ui_state.selected_project_id = Some((backend, id));
                self.ui_state.selected_session_id = None;
                self.ui_state.selected_backend_connected = self.backend_is_connected(backend);
                self.ui_state.selected_backend_capabilities = self.backend_capabilities(backend);
            }
            Some(SelKind::Worktree(id, project_id)) => {
                let backend = self.backend_of_session(id);
                self.ui_state.selected_session_id = Some(SessionRef::new(backend, id));
                self.ui_state.selected_project_id = Some((backend, project_id));
                self.ui_state.selected_backend_connected = self.backend_is_connected(backend);
                self.ui_state.selected_backend_capabilities = self.backend_capabilities(backend);
            }
            Some(SelKind::Clear) => {
                self.ui_state.selected_session_id = None;
                self.ui_state.selected_project_id = None;
                self.ui_state.selected_backend_connected = true;
                self.ui_state.selected_backend_capabilities =
                    crate::backend::BackendCapabilities::LOCAL;
            }
            Some(SelKind::Keep) | None => {}
        }

        let now_on_project = self.ui_state.selected_session_id.is_none()
            && self.ui_state.selected_project_id.is_some();

        // Auto-switch pane when transitioning between project and session
        if now_on_project && !was_on_project {
            // Transitioning to a project: Preview → Shell
            if self.ui_state.right_pane_view == RightPaneView::Preview {
                self.ui_state.right_pane_view = RightPaneView::Shell;
                self.ui_state.clear_right_pane = true;
            }
        } else if !now_on_project && was_on_project {
            // Transitioning to a session: Shell → Preview
            if self.ui_state.right_pane_view == RightPaneView::Shell {
                self.ui_state.right_pane_view = RightPaneView::Preview;
                self.ui_state.clear_right_pane = true;
            }
        }

        // Fetch info pane data if applicable
        self.spawn_info_fetch();
    }

    /// Get mutable reference to the active pane's scroll state
    pub(super) fn active_pane_state(&mut self) -> &mut PreviewState {
        match self.ui_state.right_pane_view {
            RightPaneView::Preview => &mut self.ui_state.preview_state,
            RightPaneView::Info => &mut self.ui_state.info_state,
            RightPaneView::Shell => &mut self.ui_state.shell_state,
        }
    }

    /// Map a mouse `(col, row)` in absolute terminal coordinates to a row in
    /// the session list. Returns `None` if the position is outside the list
    /// area or maps past the last rendered item.
    pub(super) fn list_index_at(&self, col: u16, row: u16) -> Option<usize> {
        list_index_at(
            col,
            row,
            self.ui_state.terminal_size,
            self.ui_state.left_pane_pct,
            self.ui_state.list_state.list_state.offset(),
            self.ui_state.list_items.len(),
        )
    }

    /// Scroll the pane under the given mouse column position
    pub(super) fn scroll_pane_at(&mut self, col: u16, direction: ScrollDirection) {
        let size = self.ui_state.terminal_size;
        if size.width == 0 || size.height == 0 {
            return;
        }

        // Recompute the same content_area as render()
        let content_area = Rect {
            x: size.x + 1,
            y: size.y + 1,
            width: size.width.saturating_sub(2),
            height: size.height.saturating_sub(3),
        };

        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(self.ui_state.left_pane_pct),
                Constraint::Percentage(100 - self.ui_state.left_pane_pct),
            ])
            .split(content_area);

        let lines_per_tick: u16 = 3;

        if col < main_chunks[0].right() {
            // Left pane: scroll the session list selection
            match direction {
                ScrollDirection::Up => self.ui_state.list_state.previous(),
                ScrollDirection::Down => self.ui_state.list_state.next(),
            }
            self.update_selection();
        } else {
            // Right pane: scroll content
            match direction {
                ScrollDirection::Up => self.active_pane_state().scroll_up(lines_per_tick),
                ScrollDirection::Down => self.active_pane_state().scroll_down(lines_per_tick),
            }
        }
    }

    /// Jump the selection to the session with the given 1-based number,
    /// update the selection state, and refresh the preview pane.
    /// Does nothing if the number is out of range.
    /// Numbering matches `TreeList::to_list_items` — the Nth `Worktree` variant.
    pub(super) fn jump_to_session_number(&mut self, number: usize) {
        if let Some(idx) = session_number_to_list_index(&self.ui_state.list_items, number) {
            self.ui_state.list_state.list_state.select(Some(idx));
            self.update_selection();
            self.ui_state.preview_update_spawned_at = None;
            self.spawn_preview_update();
        }
    }

    /// Check if a project (not a session) is currently selected
    pub(super) fn is_project_selected(&self) -> bool {
        self.ui_state.selected_session_id.is_none() && self.ui_state.selected_project_id.is_some()
    }

    /// Move the tree cursor to the `Worktree` row for `session_id` and sync
    /// selection state. No-op (returns `false`) if the session has no row in
    /// the current `list_items` — e.g. it was deleted. Callers that want the
    /// preview pane to repaint immediately should follow with
    /// `spawn_preview_update()`.
    pub(super) fn select_session_in_tree(&mut self, session_id: SessionId) -> bool {
        match worktree_list_index(&self.ui_state.list_items, session_id) {
            Some(idx) => {
                self.ui_state.list_state.select(Some(idx));
                self.update_selection();
                true
            }
            None => false,
        }
    }

    /// Resolve a tmux session name (primary or paired shell) to its session
    /// and focus it in the tree, repainting the preview pane. Used on the way
    /// out of an attach so the tree lands on the session the user just left —
    /// which, after the in-session switcher, may differ from the one they
    /// entered. Prefers the attached `backend`'s view before scanning the rest,
    /// since tmux session names can collide across machines. No-op if the
    /// session no longer exists.
    pub(super) async fn focus_session_in_tree(&mut self, backend: BackendId, tmux_name: &str) {
        // A shell pane's tmux session is named `<primary>-sh`; match either.
        let primary = tmux_name.strip_suffix("-sh").unwrap_or(tmux_name);
        let matches = |s: &crate::api::SessionInfo| {
            s.tmux_session_name == primary || s.tmux_session_name == tmux_name
        };
        let session_id = self
            .view_for(backend)
            .snapshot
            .sessions
            .iter()
            .find(|s| matches(s))
            .map(|s| s.session_id)
            .or_else(|| {
                self.backends.iter().find_map(|h| {
                    h.view
                        .snapshot
                        .sessions
                        .iter()
                        .find(|s| matches(s))
                        .map(|s| s.session_id)
                })
            });
        if let Some(id) = session_id
            && self.select_session_in_tree(id)
        {
            self.ui_state.preview_update_spawned_at = None;
            self.spawn_preview_update();
        }
    }
}

/// Find the flat `list_items` index of the `Worktree` row for `session_id`,
/// or `None` if no such row is present.
pub(super) fn worktree_list_index(
    items: &[SessionListItem],
    session_id: SessionId,
) -> Option<usize> {
    items
        .iter()
        .position(|item| matches!(item, SessionListItem::Worktree { id, .. } if *id == session_id))
}

/// Pure mapping from absolute mouse coordinates to a list item index.
///
/// Mirrors the layout in `App::render` (see `render.rs`): the content area
/// inset by 1 on each side and 3 on the bottom (for status bar), split
/// horizontally by `left_pane_pct`, then the left column is split vertically
/// into a 1-row heading and the list below it. The list itself has no
/// border, so the list's top-left maps directly to item index `offset`.
pub(super) fn list_index_at(
    col: u16,
    row: u16,
    terminal_size: Rect,
    left_pane_pct: u16,
    offset: usize,
    item_count: usize,
) -> Option<usize> {
    if terminal_size.width == 0 || terminal_size.height == 0 {
        return None;
    }

    let content_area = Rect {
        x: terminal_size.x + 1,
        y: terminal_size.y + 1,
        width: terminal_size.width.saturating_sub(2),
        height: terminal_size.height.saturating_sub(3),
    };

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left_pane_pct),
            Constraint::Percentage(100 - left_pane_pct),
        ])
        .split(content_area);
    let left = main_chunks[0];

    // Left pane: 1-line heading then the list. The list area starts at
    // `left.y + 1` and runs to `left.bottom()`.
    let list_y = left.y.checked_add(1)?;
    if col < left.x || col >= left.right() {
        return None;
    }
    if row < list_y || row >= left.bottom() {
        return None;
    }

    let visible = (row - list_y) as usize;
    let idx = offset.checked_add(visible)?;
    if idx >= item_count {
        return None;
    }
    Some(idx)
}

/// Name of the section containing the list row at `idx` — the nearest
/// `SectionHeader` at or above it. Returns `None` when the row sits above any
/// header (non-sectioned view modes render no headers) or under the implicit
/// "In Progress" catch-all, where new sessions land by default anyway.
pub(super) fn section_at(items: &[SessionListItem], idx: usize) -> Option<String> {
    items
        .get(..=idx)?
        .iter()
        .rev()
        .find_map(|item| match item {
            SessionListItem::SectionHeader { name, .. } => Some(name.clone()),
            _ => None,
        })
        .filter(|name| name != crate::session::IN_PROGRESS)
}

/// Map a 1-based session number to its index in the flat list_items vec.
/// Returns None if the number is out of range.
pub(super) fn session_number_to_list_index(
    items: &[SessionListItem],
    number: usize,
) -> Option<usize> {
    let mut count = 0usize;
    for (idx, item) in items.iter().enumerate() {
        if matches!(item, SessionListItem::Worktree { .. }) {
            count += 1;
            if count == number {
                return Some(idx);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typical 80x24 terminal split 30/70 with the standard 1-cell margins
    /// — used as a fixture for the `list_index_at` tests below.
    fn term() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    /// Returns the rendered list rect for the 80x24 / 30% fixture so tests
    /// can compute valid click positions without duplicating layout math.
    fn list_rect() -> Rect {
        let size = term();
        let content_area = Rect {
            x: size.x + 1,
            y: size.y + 1,
            width: size.width.saturating_sub(2),
            height: size.height.saturating_sub(3),
        };
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(content_area);
        let left = chunks[0];
        Rect::new(left.x, left.y + 1, left.width, left.height - 1)
    }

    fn header(name: &str) -> SessionListItem {
        SessionListItem::SectionHeader {
            name: name.to_string(),
            count: 1,
            collapsed: false,
            max_sessions: None,
        }
    }

    fn project_row() -> SessionListItem {
        SessionListItem::Project {
            id: crate::session::ProjectId::new(),
            name: "proj".into(),
            repo_path: std::path::PathBuf::from("/dev/null/unused"),
            main_branch: "main".into(),
            worktree_count: 1,
            nested: true,
        }
    }

    #[test]
    fn section_at_finds_nearest_header_above() {
        let items = vec![
            header("Awaiting Review"),
            project_row(),
            SessionListItem::Spacer,
            header("Self Review"),
            project_row(),
        ];
        assert_eq!(section_at(&items, 4).as_deref(), Some("Self Review"));
        assert_eq!(section_at(&items, 1).as_deref(), Some("Awaiting Review"));
    }

    #[test]
    fn section_at_on_header_row_returns_that_section() {
        let items = vec![header("Self Review"), project_row()];
        assert_eq!(section_at(&items, 0).as_deref(), Some("Self Review"));
    }

    #[test]
    fn section_at_in_progress_catchall_is_none() {
        let items = vec![crate::session::IN_PROGRESS, "Self Review"]
            .into_iter()
            .map(header)
            .collect::<Vec<_>>();
        assert_eq!(section_at(&items, 0), None);
    }

    #[test]
    fn section_at_without_headers_is_none() {
        // Non-sectioned view modes render no SectionHeader rows at all.
        let items = vec![project_row(), project_row()];
        assert_eq!(section_at(&items, 1), None);
    }

    #[test]
    fn section_at_out_of_bounds_is_none() {
        let items = vec![header("Self Review")];
        assert_eq!(section_at(&items, 5), None);
    }

    #[test]
    fn list_index_at_first_row_with_no_offset_is_zero() {
        let lr = list_rect();
        assert_eq!(list_index_at(lr.x, lr.y, term(), 30, 0, 10), Some(0));
    }

    #[test]
    fn list_index_at_adds_scroll_offset() {
        let lr = list_rect();
        // Second visible row, list scrolled by 5 → item index 6
        assert_eq!(list_index_at(lr.x, lr.y + 1, term(), 30, 5, 20), Some(6));
    }

    #[test]
    fn list_index_at_returns_none_on_heading_row() {
        // The heading row sits at `list_rect().y - 1` (the first row of the
        // left pane, before the list). A click there must not select a row.
        let lr = list_rect();
        assert_eq!(list_index_at(lr.x, lr.y - 1, term(), 30, 0, 10), None);
    }

    #[test]
    fn list_index_at_returns_none_beyond_item_count() {
        let lr = list_rect();
        // Click far enough down that the item index exceeds item_count.
        assert_eq!(list_index_at(lr.x, lr.y, term(), 30, 0, 0), None);
        assert_eq!(list_index_at(lr.x, lr.y + 5, term(), 30, 0, 3), None);
    }

    #[test]
    fn list_index_at_returns_none_outside_left_pane() {
        let lr = list_rect();
        // Click in the right pane — should map to nothing in the list.
        let right_col = lr.right() + 1;
        assert_eq!(list_index_at(right_col, lr.y, term(), 30, 0, 10), None);
    }

    #[test]
    fn list_index_at_returns_none_below_content_area() {
        let lr = list_rect();
        // Click on the status-bar row at the bottom of the terminal.
        let below = term().bottom() - 1;
        assert!(below >= lr.bottom());
        assert_eq!(list_index_at(lr.x, below, term(), 30, 0, 100), None);
    }

    #[test]
    fn list_index_at_zero_size_terminal_returns_none() {
        let size = Rect::new(0, 0, 0, 0);
        assert_eq!(list_index_at(0, 0, size, 30, 0, 10), None);
    }
}
