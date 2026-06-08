//! Full-screen review-diff-and-annotate view.
//!
//! Presentation only: [`DiffReviewState`] holds what's on screen and is opened
//! via `CommanderService::open_review`. The view is hosted as a maximised
//! modal (`Modal::ReviewDiff`); all diff composition, parsing, and annotation
//! logic lives in the library.

use super::*;
use crossterm::event::KeyEvent;

use crate::annotation::{Annotation, AnnotationSide, AnnotationStatus};
use crate::api::AnnotationDraft;
use crate::git::{DiffLine, FileDiff, FileStatus, Hunk, LineOrigin, ParsedDiff};
use crate::tui::syntax_highlight::highlight_line;
use crate::tui::theme::{ColorMode, ReviewPalette};

/// Rough viewport height used to keep the cursor visible while scrolling. The
/// renderer doesn't report its height back to the state, so we approximate
/// (same pragmatic approach as the checkout-branch list).
const BODY_VIEWPORT: usize = 20;

/// Which column has keyboard focus inside the review view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFocus {
    FileList,
    Body,
}

/// How the diff body is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewLayout {
    /// GitHub-style unified inline diff (default).
    Inline,
    /// Old | new split columns.
    SideBySide,
}

/// An in-progress comment, captured against a selectable-line range.
#[derive(Debug, Clone)]
pub struct CommentDraft {
    pub text: String,
    /// Inclusive selectable-line index range the comment applies to.
    pub range: (usize, usize),
}

/// State backing the full-screen review view.
#[derive(Debug, Clone)]
pub struct DiffReviewState {
    pub session_id: SessionId,
    pub title: String,
    /// Base the diff was computed against (branch/sha/HEAD), for the header.
    pub base: String,
    pub diff: ParsedDiff,
    pub annotations: Vec<Annotation>,
    /// Index into `diff.files` of the file shown in the body.
    pub selected_file: usize,
    /// First visible body row (scroll offset).
    pub scroll: u16,
    pub focus: ReviewFocus,
    /// Cursor position as a selectable-line index within the current file.
    pub cursor: usize,
    /// `Some(anchor)` while in visual (range-select) mode; the active end is
    /// `cursor`.
    pub visual_anchor: Option<usize>,
    /// `Some` while the comment box is open.
    pub comment: Option<CommentDraft>,
    pub layout: ReviewLayout,
    /// File tree built from the diff's paths (single-child directory chains
    /// compressed, lazygit-style).
    file_tree: Vec<TreeNode>,
    /// Paths of directory nodes the user has collapsed.
    collapsed: HashSet<String>,
    /// Cursor over the flattened, currently-visible tree rows.
    tree_cursor: usize,
    /// First visible tree row (scroll offset for the file pane).
    tree_scroll: u16,
}

/// A node in the file tree: either a directory (with children) or a file leaf
/// (`file_index` into `diff.files`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeNode {
    /// Segment label (compressed directories join with `/`).
    name: String,
    /// Full path of this node, used as the collapse-set key.
    path: String,
    file_index: Option<usize>,
    children: Vec<TreeNode>,
}

/// A flattened, visible tree row for rendering and navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeRow {
    Dir {
        depth: usize,
        path: String,
        name: String,
        collapsed: bool,
    },
    File {
        depth: usize,
        index: usize,
        name: String,
    },
}

impl DiffReviewState {
    pub fn new(
        session_id: SessionId,
        title: String,
        base: String,
        diff: ParsedDiff,
        annotations: Vec<Annotation>,
    ) -> Self {
        let file_tree = build_file_tree(&diff.files);
        // Body starts on the first file in tree order so it shows something.
        let selected_file = first_file_index(&file_tree).unwrap_or(0);
        Self {
            session_id,
            title,
            base,
            diff,
            annotations,
            selected_file,
            scroll: 0,
            focus: ReviewFocus::FileList,
            cursor: 0,
            visual_anchor: None,
            comment: None,
            layout: ReviewLayout::Inline,
            file_tree,
            collapsed: HashSet::new(),
            tree_cursor: 0,
            tree_scroll: 0,
        }
    }

    /// The currently-visible tree rows (respecting collapsed directories).
    fn visible_rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        flatten_tree(&self.file_tree, 0, &self.collapsed, &mut rows);
        rows
    }

    /// Point the body at `idx`, resetting the body cursor/scroll/selection if
    /// it actually changed.
    fn set_body_file(&mut self, idx: usize) {
        if self.selected_file != idx {
            self.selected_file = idx;
            self.scroll = 0;
            self.cursor = 0;
            self.visual_anchor = None;
        }
    }

    /// Move the tree cursor over visible rows; landing on a file shows it.
    fn tree_move(&mut self, down: bool) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        self.tree_cursor = if down {
            (self.tree_cursor + 1).min(rows.len() - 1)
        } else {
            self.tree_cursor.saturating_sub(1)
        };
        self.follow_tree_cursor(rows.len());
        if let Some(TreeRow::File { index, .. }) = rows.get(self.tree_cursor) {
            self.set_body_file(*index);
        }
    }

    /// Enter/Space on a directory row toggles its collapsed state.
    fn tree_activate(&mut self) {
        let rows = self.visible_rows();
        if let Some(TreeRow::Dir {
            path, collapsed, ..
        }) = rows.get(self.tree_cursor)
        {
            if *collapsed {
                self.collapsed.remove(path);
            } else {
                self.collapsed.insert(path.clone());
            }
            // The toggled dir keeps its index; clamp just in case.
            let len = self.visible_rows().len();
            self.tree_cursor = self.tree_cursor.min(len.saturating_sub(1));
        }
    }

    /// Keep the tree cursor within the (approximate) file-pane viewport.
    fn follow_tree_cursor(&mut self, _len: usize) {
        let row = self.tree_cursor as u16;
        let bottom = self.tree_scroll + BODY_VIEWPORT as u16;
        if row < self.tree_scroll {
            self.tree_scroll = row;
        } else if row >= bottom {
            self.tree_scroll = row.saturating_sub(BODY_VIEWPORT as u16 - 1);
        }
    }

    /// Sync the tree cursor onto the visible row for `selected_file`, if shown.
    fn sync_tree_cursor_to_file(&mut self) {
        let rows = self.visible_rows();
        if let Some(pos) = rows
            .iter()
            .position(|r| matches!(r, TreeRow::File { index, .. } if *index == self.selected_file))
        {
            self.tree_cursor = pos;
            self.follow_tree_cursor(rows.len());
        }
    }

    fn toggle_layout(&mut self) {
        self.layout = match self.layout {
            ReviewLayout::Inline => ReviewLayout::SideBySide,
            ReviewLayout::SideBySide => ReviewLayout::Inline,
        };
    }

    fn current_file(&self) -> Option<&FileDiff> {
        self.diff.files.get(self.selected_file)
    }

    /// The current file's diff lines in render order (selection operates over
    /// these; hunk headers are not selectable).
    fn selectable_lines(&self) -> Vec<&DiffLine> {
        self.current_file()
            .map(|f| f.hunks.iter().flat_map(|h| h.lines.iter()).collect())
            .unwrap_or_default()
    }

    fn selectable_count(&self) -> usize {
        self.current_file()
            .map(|f| f.hunks.iter().map(|h| h.lines.len()).sum())
            .unwrap_or(0)
    }

    /// Number of not-yet-applied annotations anchored to `file`.
    fn annotation_count(&self, file: &str) -> usize {
        self.annotations
            .iter()
            .filter(|a| a.status != AnnotationStatus::Applied && a.file == file)
            .count()
    }

    /// Inclusive selection range over selectable-line indices: the visual
    /// anchor..cursor when selecting, else the single cursor line.
    fn selection(&self) -> (usize, usize) {
        match self.visual_anchor {
            Some(anchor) => (anchor.min(self.cursor), anchor.max(self.cursor)),
            None => (self.cursor, self.cursor),
        }
    }

    /// Jump the body to the next file (in diff order), syncing the tree cursor.
    pub fn next_file(&mut self) {
        if self.diff.files.is_empty() {
            return;
        }
        self.set_body_file((self.selected_file + 1).min(self.diff.files.len() - 1));
        self.sync_tree_cursor_to_file();
    }

    /// Jump the body to the previous file (in diff order).
    pub fn prev_file(&mut self) {
        self.set_body_file(self.selected_file.saturating_sub(1));
        self.sync_tree_cursor_to_file();
    }

    /// Move the body cursor by one line, clamped, keeping it visible.
    fn move_cursor(&mut self, down: bool) {
        let count = self.selectable_count();
        if count == 0 {
            return;
        }
        if down {
            self.cursor = (self.cursor + 1).min(count - 1);
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
        self.follow_cursor();
    }

    /// Adjust `scroll` so the cursor's body row stays within the viewport.
    fn follow_cursor(&mut self) {
        let row = self.body_row_of(self.cursor) as u16;
        let top = self.scroll;
        let bottom = self.scroll + BODY_VIEWPORT as u16;
        if row < top {
            self.scroll = row;
        } else if row >= bottom {
            self.scroll = row.saturating_sub(BODY_VIEWPORT as u16 - 1);
        }
    }

    /// Body row index (counting hunk headers) of selectable line `idx`.
    fn body_row_of(&self, idx: usize) -> usize {
        let Some(file) = self.current_file() else {
            return 0;
        };
        let mut row = 0;
        let mut sel = 0;
        for hunk in &file.hunks {
            row += 1; // header
            for _ in &hunk.lines {
                if sel == idx {
                    return row;
                }
                row += 1;
                sel += 1;
            }
        }
        row
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            ReviewFocus::FileList => ReviewFocus::Body,
            ReviewFocus::Body => ReviewFocus::FileList,
        };
    }

    /// Open the comment box for the current selection (Enter / right-click).
    /// No-op (returns false) when the file has no diff lines or a comment is
    /// already open.
    pub fn begin_comment(&mut self) -> bool {
        if self.selectable_count() == 0 || self.comment.is_some() {
            return false;
        }
        self.focus = ReviewFocus::Body;
        self.comment = Some(CommentDraft {
            text: String::new(),
            range: self.selection(),
        });
        true
    }

    /// Total body rows (hunk headers + lines) for the current file.
    fn total_body_rows(&self) -> usize {
        self.current_file()
            .map(|f| f.hunks.iter().map(|h| h.lines.len() + 1).sum())
            .unwrap_or(0)
    }

    /// Selectable-line index at body row `body_row` (`None` for a header row or
    /// out of range) — the inverse of [`Self::body_row_of`].
    fn selectable_at_body_row(&self, body_row: usize) -> Option<usize> {
        let file = self.current_file()?;
        let mut row = 0;
        let mut sel = 0;
        for hunk in &file.hunks {
            if row == body_row {
                return None; // header row
            }
            row += 1;
            for _ in &hunk.lines {
                if row == body_row {
                    return Some(sel);
                }
                row += 1;
                sel += 1;
            }
        }
        None
    }

    /// Scroll the body by one row (free of the cursor), for mouse wheel.
    pub fn wheel(&mut self, down: bool) {
        let max = self.total_body_rows().saturating_sub(1) as u16;
        self.scroll = if down {
            (self.scroll + 1).min(max)
        } else {
            self.scroll.saturating_sub(1)
        };
    }

    /// Left-click at a screen position: focus the body and move the cursor to
    /// the clicked diff line (clearing any selection).
    pub fn click_at(&mut self, col: u16, row: u16, body: Rect) {
        let Some(body_row) = self.body_row_at(col, row, body) else {
            return;
        };
        self.focus = ReviewFocus::Body;
        self.visual_anchor = None;
        // Row→line mapping is computed for the inline layout; in side-by-side
        // the row structure differs, so a click only moves focus there.
        if self.layout == ReviewLayout::Inline
            && let Some(idx) = self.selectable_at_body_row(body_row)
        {
            self.cursor = idx;
        }
    }

    /// Left-drag to a screen position: begin a selection at the press point (if
    /// not already selecting) and extend it to the dragged line.
    pub fn drag_at(&mut self, col: u16, row: u16, body: Rect) {
        if self.layout != ReviewLayout::Inline {
            return;
        }
        let Some(body_row) = self.body_row_at(col, row, body) else {
            return;
        };
        if let Some(idx) = self.selectable_at_body_row(body_row) {
            if self.visual_anchor.is_none() {
                self.visual_anchor = Some(self.cursor);
            }
            self.cursor = idx;
        }
    }

    /// Map a screen position inside `body` to a body-row index (accounting for
    /// scroll), or `None` if outside.
    fn body_row_at(&self, col: u16, row: u16, body: Rect) -> Option<usize> {
        let inside = col >= body.x
            && col < body.x + body.width
            && row >= body.y
            && row < body.y + body.height;
        if !inside {
            return None;
        }
        Some((row - body.y) as usize + self.scroll as usize)
    }

    /// Enter visual mode at the cursor, or cancel it if already active.
    fn toggle_visual(&mut self) {
        self.visual_anchor = match self.visual_anchor {
            Some(_) => None,
            None => Some(self.cursor),
        };
    }

    /// Build an annotation draft from a selectable-line range plus comment
    /// text. Picks the New side unless the selection is purely deletions, and
    /// captures the snippet/line range from that side's lines only (so it
    /// re-anchors cleanly).
    fn build_draft(&self, range: (usize, usize), comment: String) -> Option<AnnotationDraft> {
        let file = self.current_file()?;
        let lines = self.selectable_lines();
        let (lo, hi) = range;
        let sel = lines.get(lo..=hi.min(lines.len().saturating_sub(1)))?;
        if sel.is_empty() {
            return None;
        }

        let any_new = sel.iter().any(|l| l.new_lineno.is_some());
        let side = if any_new {
            AnnotationSide::New
        } else {
            AnnotationSide::Old
        };

        let mut nums = Vec::new();
        let mut contents = Vec::new();
        for line in sel {
            let lineno = match side {
                AnnotationSide::New => line.new_lineno,
                AnnotationSide::Old => line.old_lineno,
            };
            if let Some(n) = lineno {
                nums.push(n);
                contents.push(line.content.clone());
            }
        }
        if nums.is_empty() {
            return None;
        }

        Some(AnnotationDraft {
            file: file.display_path().to_string(),
            side,
            line_range: (*nums.iter().min().unwrap(), *nums.iter().max().unwrap()),
            snippet: contents.join("\n"),
            comment,
        })
    }

    /// Id of a not-yet-applied annotation covering the cursor line, if any.
    fn annotation_at_cursor(&self) -> Option<uuid::Uuid> {
        let file = self.current_file()?;
        let line = *self.selectable_lines().get(self.cursor)?;
        self.annotations
            .iter()
            .filter(|a| a.status != AnnotationStatus::Applied && a.file == file.display_path())
            .find(|a| {
                let lineno = match a.side {
                    AnnotationSide::New => line.new_lineno,
                    AnnotationSide::Old => line.old_lineno,
                };
                lineno.is_some_and(|n| a.line_range.0 <= n && n <= a.line_range.1)
            })
            .map(|a| a.id)
    }

    /// Whether selectable line `idx` is covered by a non-applied annotation,
    /// and whether any such annotation is drifted.
    fn annotation_marker(&self, idx: usize) -> Option<bool> {
        let file = self.current_file()?;
        let line = *self.selectable_lines().get(idx)?;
        let mut drifted = false;
        let mut found = false;
        for a in self
            .annotations
            .iter()
            .filter(|a| a.status != AnnotationStatus::Applied && a.file == file.display_path())
        {
            let lineno = match a.side {
                AnnotationSide::New => line.new_lineno,
                AnnotationSide::Old => line.old_lineno,
            };
            if lineno.is_some_and(|n| a.line_range.0 <= n && n <= a.line_range.1) {
                found = true;
                drifted |= a.status == AnnotationStatus::Drifted;
            }
        }
        found.then_some(drifted)
    }
}

impl App {
    /// Open the review view for the selected session.
    pub(super) async fn handle_open_review(&mut self) {
        let Some(session_id) = self.ui_state.selected_session_id else {
            self.set_review_status("Select a session first");
            return;
        };

        let title = {
            let state = self.service.store().read().await;
            state
                .sessions
                .get(&session_id)
                .map(|s| s.title.clone())
                .unwrap_or_default()
        };

        match self.service.open_review(&session_id).await {
            Ok(snapshot) => {
                if snapshot.diff.is_empty() {
                    self.set_review_status("No changes to review");
                    return;
                }
                let state = DiffReviewState::new(
                    session_id,
                    title,
                    snapshot.base,
                    snapshot.diff,
                    snapshot.annotations,
                );
                self.ui_state.modal = Modal::ReviewDiff(Box::new(state));
            }
            Err(e) => {
                self.ui_state.modal = Modal::Error {
                    message: format!("Failed to open review: {e}"),
                };
            }
        }
    }

    fn set_review_status(&mut self, msg: &str) {
        self.ui_state.status_message =
            Some((msg.to_string(), Instant::now() + Duration::from_secs(3)));
    }

    /// Reload the session's annotations and re-anchor them against the current
    /// diff (used after create/delete/apply, which don't change the diff).
    async fn reload_review_annotations(&self, state: &mut DiffReviewState) {
        let mut anns = self
            .service
            .list_annotations(&state.session_id)
            .await
            .unwrap_or_default();
        crate::annotation::reanchor_annotations(&mut anns, &state.diff);
        state.annotations = anns;
    }

    /// Handle a key while the review view is open. `state` has been moved out
    /// of `self.ui_state.modal`; it is put back unless the view is closed.
    pub(super) async fn handle_review_key(
        &mut self,
        key: KeyEvent,
        mut state: Box<DiffReviewState>,
    ) {
        use crossterm::event::KeyCode;

        // Comment box captures all input while open.
        if state.comment.is_some() {
            self.handle_review_comment_key(key, &mut state).await;
            self.ui_state.modal = Modal::ReviewDiff(state);
            return;
        }

        match key.code {
            // Esc cancels an in-progress selection first; otherwise closes
            // (the modal was already replaced with None on extraction).
            KeyCode::Esc if state.visual_anchor.is_none() => return,
            KeyCode::Esc => state.visual_anchor = None,
            KeyCode::Tab => state.toggle_focus(),
            KeyCode::Char(']') => state.next_file(),
            KeyCode::Char('[') => state.prev_file(),
            KeyCode::Down | KeyCode::Char('j') => match state.focus {
                ReviewFocus::FileList => state.tree_move(true),
                ReviewFocus::Body => state.move_cursor(true),
            },
            KeyCode::Up | KeyCode::Char('k') => match state.focus {
                ReviewFocus::FileList => state.tree_move(false),
                ReviewFocus::Body => state.move_cursor(false),
            },
            KeyCode::Char('t') => state.toggle_layout(),
            KeyCode::Char('v') if state.focus == ReviewFocus::Body => state.toggle_visual(),
            // Enter: toggle a directory in the tree, or open the comment box in
            // the body.
            KeyCode::Enter if state.focus == ReviewFocus::FileList => state.tree_activate(),
            KeyCode::Enter if state.focus == ReviewFocus::Body => {
                state.begin_comment();
            }
            KeyCode::Char('d') if state.focus == ReviewFocus::Body => {
                if let Some(id) = state.annotation_at_cursor() {
                    if let Err(e) = self.service.delete_annotation(&state.session_id, id).await {
                        self.set_review_status(&format!("Delete failed: {e}"));
                    } else {
                        self.reload_review_annotations(&mut state).await;
                    }
                } else {
                    self.set_review_status("No annotation on this line");
                }
            }
            KeyCode::Char('a') => self.apply_review(&mut state).await,
            _ => {}
        }
        self.ui_state.modal = Modal::ReviewDiff(state);
    }

    async fn handle_review_comment_key(&mut self, key: KeyEvent, state: &mut DiffReviewState) {
        use crossterm::event::KeyCode;
        let Some(draft) = state.comment.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                state.comment = None;
            }
            KeyCode::Enter => {
                let draft = state.comment.take().expect("comment present");
                if draft.text.trim().is_empty() {
                    return;
                }
                if let Some(ann) = state.build_draft(draft.range, draft.text) {
                    match self.service.create_annotation(&state.session_id, ann).await {
                        Ok(_) => {
                            state.visual_anchor = None;
                            self.reload_review_annotations(state).await;
                        }
                        Err(e) => self.set_review_status(&format!("Annotation failed: {e}")),
                    }
                }
            }
            KeyCode::Backspace => {
                draft.text.pop();
            }
            KeyCode::Char(c) => draft.text.push(c),
            _ => {}
        }
    }

    /// Apply staged annotations and report the outcome.
    async fn apply_review(&mut self, state: &mut DiffReviewState) {
        use crate::annotation::ApplyOutcome;
        match self.service.apply_annotations(&state.session_id).await {
            Ok(ApplyOutcome::Nothing) => self.set_review_status("No staged annotations to apply"),
            Ok(ApplyOutcome::Blocked { drifted }) => self.set_review_status(&format!(
                "{} drifted annotation(s) block apply — review or delete them",
                drifted.len()
            )),
            Ok(ApplyOutcome::Applied { count, .. }) => {
                self.reload_review_annotations(state).await;
                self.set_review_status(&format!("Sent {count} annotation(s) to the agent"));
            }
            Ok(ApplyOutcome::Deferred { count, .. }) => {
                self.reload_review_annotations(state).await;
                self.set_review_status(&format!(
                    "{count} annotation(s) queued — agent busy or stopped"
                ));
            }
            Err(e) => self.set_review_status(&format!("Apply failed: {e}")),
        }
    }

    /// Render the full-screen review view.
    pub(super) fn render_review_modal(
        &self,
        frame: &mut Frame,
        area: Rect,
        state: &DiffReviewState,
    ) {
        frame.render_widget(Clear, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(34), Constraint::Min(0)])
            .split(rows[0]);

        self.render_review_file_list(frame, cols[0], state);
        self.render_review_body(frame, cols[1], state);

        let hint = if state.comment.is_some() {
            " type comment · Enter save · Esc cancel "
        } else if state.visual_anchor.is_some() {
            " ↑↓ extend · Enter/right-click comment · v/Esc cancel selection "
        } else if state.focus == ReviewFocus::FileList {
            " ↑↓/jk move · Enter expand/collapse · [ ] file · Tab to diff · a apply · Esc close "
        } else {
            " ↑↓/jk move · v select · Enter comment · d delete · a apply · t layout · Tab files · Esc close "
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default().fg(Color::DarkGray),
            ))),
            rows[1],
        );

        if state.comment.is_some() {
            self.render_review_comment_box(frame, cols[1], state);
        }
    }

    fn render_review_file_list(&self, frame: &mut Frame, area: Rect, state: &DiffReviewState) {
        let focused = state.focus == ReviewFocus::FileList;
        let border = if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let rows = state.visible_rows();
        let mut lines: Vec<Line> = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let on_cursor = focused && i == state.tree_cursor;
            let line = match row {
                TreeRow::Dir {
                    depth,
                    name,
                    collapsed,
                    ..
                } => {
                    let indent = "  ".repeat(*depth);
                    let chevron = if *collapsed { '▶' } else { '▼' };
                    let style = Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD);
                    let style = if on_cursor {
                        style.add_modifier(Modifier::REVERSED)
                    } else {
                        style
                    };
                    Line::from(Span::styled(format!("{indent}{chevron} {name}"), style))
                }
                TreeRow::File { depth, index, name } => {
                    let file = &state.diff.files[*index];
                    let indent = "  ".repeat(*depth);
                    let marker = file_status_marker(file.status);
                    let count = state.annotation_count(file.display_path());
                    let badge = if count > 0 {
                        format!(" ✎{count}")
                    } else {
                        String::new()
                    };
                    let mut style = Style::default().fg(file_status_color(file.status));
                    if on_cursor {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    Line::from(Span::styled(
                        format!("{indent}  {marker} {name}{badge}"),
                        style,
                    ))
                }
            };
            lines.push(line);
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(format!(" Files ({}) ", state.diff.files.len()));
        frame.render_widget(
            Paragraph::new(lines)
                .block(block)
                .scroll((state.tree_scroll, 0)),
            area,
        );
    }

    fn render_review_body(&self, frame: &mut Frame, area: Rect, state: &DiffReviewState) {
        let focused = state.focus == ReviewFocus::Body;
        let border = if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let title = match state.current_file() {
            Some(f) => format!(" {} — vs {} ", f.display_path(), state.base),
            None => format!(" review — vs {} ", state.base),
        };

        let pal = self.theme.review_palette();
        // Syntax highlighting emits RGB foregrounds, so only apply it on
        // true-color terminals; otherwise fall back to the palette text colour.
        let highlight = self.theme.mode == ColorMode::TrueColor;
        let ext = state
            .current_file()
            .map(|f| file_extension(f.display_path()).to_string())
            .unwrap_or_default();
        let width = area.width.saturating_sub(2) as usize;
        let lines = match state.layout {
            ReviewLayout::Inline => review_body_lines(state, focused, &pal, &ext, highlight, width),
            ReviewLayout::SideBySide => {
                review_body_lines_side_by_side(state, focused, &pal, &ext, highlight, width)
            }
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(title);
        frame.render_widget(
            Paragraph::new(lines).block(block).scroll((state.scroll, 0)),
            area,
        );
    }

    /// Small input box for the in-progress comment, drawn over the body.
    fn render_review_comment_box(
        &self,
        frame: &mut Frame,
        body_area: Rect,
        state: &DiffReviewState,
    ) {
        let Some(draft) = state.comment.as_ref() else {
            return;
        };
        let height = 3;
        let area = Rect {
            x: body_area.x + 2,
            y: body_area.y + body_area.height.saturating_sub(height + 1),
            width: body_area.width.saturating_sub(4),
            height,
        };
        frame.render_widget(Clear, area);
        let (lo, hi) = draft.range;
        let loc = if lo == hi {
            format!("line {}", lo + 1)
        } else {
            format!("lines {}–{}", lo + 1, hi + 1)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(format!(" Comment ({loc}) "));
        frame.render_widget(
            Paragraph::new(Line::from(format!("{}▏", draft.text))).block(block),
            area,
        );
    }
}

/// Inner rect of the diff body pane for a given modal `area` — the region a
/// mouse position maps into. Must mirror the layout in `render_review_modal`.
pub(super) fn review_body_inner_rect(area: Rect) -> Rect {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(0)])
        .split(rows[0]);
    // Inset by the body block's border.
    cols[1].inner(Margin {
        vertical: 1,
        horizontal: 1,
    })
}

/// Build a file tree from the diff's files (keyed on each file's display path),
/// then compress single-child directory chains (lazygit-style).
fn build_file_tree(files: &[FileDiff]) -> Vec<TreeNode> {
    let mut roots: Vec<TreeNode> = Vec::new();
    for (idx, file) in files.iter().enumerate() {
        let segments: Vec<&str> = file.display_path().split('/').collect();
        insert_path(&mut roots, &segments, idx, "");
    }
    for node in &mut roots {
        compress(node);
    }
    roots
}

/// Insert a file's path segments into the tree, creating directory nodes.
fn insert_path(children: &mut Vec<TreeNode>, segments: &[&str], file_index: usize, prefix: &str) {
    let Some((head, rest)) = segments.split_first() else {
        return;
    };
    let path = if prefix.is_empty() {
        head.to_string()
    } else {
        format!("{prefix}/{head}")
    };
    if rest.is_empty() {
        children.push(TreeNode {
            name: head.to_string(),
            path,
            file_index: Some(file_index),
            children: Vec::new(),
        });
        return;
    }
    let pos = children
        .iter()
        .position(|n| n.file_index.is_none() && n.name == *head);
    let idx = match pos {
        Some(i) => i,
        None => {
            children.push(TreeNode {
                name: head.to_string(),
                path: path.clone(),
                file_index: None,
                children: Vec::new(),
            });
            children.len() - 1
        }
    };
    insert_path(&mut children[idx].children, rest, file_index, &path);
}

/// Merge a directory with its sole child when that child is also a directory,
/// repeatedly, then recurse. `a` → `b` → files becomes `a/b`.
fn compress(node: &mut TreeNode) {
    while node.file_index.is_none()
        && node.children.len() == 1
        && node.children[0].file_index.is_none()
    {
        let child = node.children.remove(0);
        node.name = format!("{}/{}", node.name, child.name);
        node.path = child.path;
        node.children = child.children;
    }
    for child in &mut node.children {
        compress(child);
    }
}

/// Flatten the tree into visible rows, skipping collapsed directories' subtrees.
fn flatten_tree(
    nodes: &[TreeNode],
    depth: usize,
    collapsed: &HashSet<String>,
    out: &mut Vec<TreeRow>,
) {
    for node in nodes {
        match node.file_index {
            Some(index) => out.push(TreeRow::File {
                depth,
                index,
                name: node.name.clone(),
            }),
            None => {
                let is_collapsed = collapsed.contains(&node.path);
                out.push(TreeRow::Dir {
                    depth,
                    path: node.path.clone(),
                    name: node.name.clone(),
                    collapsed: is_collapsed,
                });
                if !is_collapsed {
                    flatten_tree(&node.children, depth + 1, collapsed, out);
                }
            }
        }
    }
}

/// Index of the first file in tree order (depth-first), if any.
fn first_file_index(nodes: &[TreeNode]) -> Option<usize> {
    for node in nodes {
        if let Some(i) = node.file_index {
            return Some(i);
        }
        if let Some(i) = first_file_index(&node.children) {
            return Some(i);
        }
    }
    None
}

/// One-character marker for a file's change status.
fn file_status_marker(status: FileStatus) -> char {
    match status {
        FileStatus::Added => 'A',
        FileStatus::Deleted => 'D',
        FileStatus::Modified => 'M',
        FileStatus::Renamed => 'R',
    }
}

/// Colour used for a file row by its change status.
fn file_status_color(status: FileStatus) -> Color {
    match status {
        FileStatus::Added => Color::Green,
        FileStatus::Deleted => Color::Red,
        FileStatus::Modified => Color::Yellow,
        FileStatus::Renamed => Color::Cyan,
    }
}

/// Build the inline-rendered body for the current file: hunk headers plus each
/// diff line with a coloured gutter, full-width add/remove background fill,
/// word-level intra-line highlight, and an annotation marker. Selected lines
/// (cursor or visual range) are reversed when the body is focused.
fn review_body_lines(
    state: &DiffReviewState,
    focused: bool,
    pal: &ReviewPalette,
    ext: &str,
    highlight: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let Some(file) = state.current_file() else {
        return Vec::new();
    };
    let (sel_lo, sel_hi) = state.selection();
    let segs = word_diff_segments(file);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut idx = 0; // selectable-line index

    for hunk in &file.hunks {
        out.push(hunk_header_line(hunk, pal, width));
        for line in &hunk.lines {
            let (line_bg, gutter_bg, emph_bg, sign, sign_fg) = match line.origin {
                LineOrigin::Addition => (
                    pal.add_bg,
                    pal.add_gutter_bg,
                    pal.add_emph_bg,
                    '+',
                    Color::Green,
                ),
                LineOrigin::Deletion => (
                    pal.del_bg,
                    pal.del_gutter_bg,
                    pal.del_emph_bg,
                    '-',
                    Color::Red,
                ),
                LineOrigin::Context => {
                    (Color::Reset, Color::Reset, Color::Reset, ' ', pal.gutter_fg)
                }
            };
            let ann = match state.annotation_marker(idx) {
                Some(true) => '⚠',
                Some(false) => '✎',
                None => ' ',
            };
            let old = lineno_str(line.old_lineno);
            let new = lineno_str(line.new_lineno);

            let mut spans = vec![
                // Bright left edge bar on changed lines.
                Span::styled(" ", Style::default().bg(emph_bg)),
                Span::styled(
                    format!("{ann}{old} {new} "),
                    Style::default().fg(pal.gutter_fg).bg(gutter_bg),
                ),
                Span::styled(sign.to_string(), Style::default().fg(sign_fg).bg(line_bg)),
            ];
            for (text, emph) in &segs[idx] {
                let bg = if *emph { emph_bg } else { line_bg };
                push_segment(&mut spans, text, ext, highlight, pal.text, bg);
            }
            let mut spans = fit_spans(spans, width, line_bg);
            if focused && idx >= sel_lo && idx <= sel_hi {
                spans = reverse_spans(spans);
            }
            out.push(Line::from(spans));
            idx += 1;
        }
    }
    out
}

/// Right-aligned 4-wide line number, or blanks when absent.
fn lineno_str(n: Option<usize>) -> String {
    n.map(|n| format!("{n:>4}"))
        .unwrap_or_else(|| "    ".to_string())
}

/// A full-width hunk-header line.
fn hunk_header_line(hunk: &Hunk, pal: &ReviewPalette, width: usize) -> Line<'static> {
    let text = format!(
        "@@ -{},{} +{},{} @@ {}",
        hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines, hunk.header
    );
    fit_spans(
        vec![Span::styled(text, Style::default().fg(pal.hunk_header))],
        width,
        Color::Reset,
    )
    .into()
}

/// Truncate a styled span list to `width` display columns (cutting the last
/// span), or right-pad it with a `pad_bg`-filled space span so the row's
/// background fills the full width.
fn fit_spans(spans: Vec<Span<'static>>, width: usize, pad_bg: Color) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= width {
            break;
        }
        let len = span.content.chars().count();
        if used + len <= width {
            used += len;
            out.push(span);
        } else {
            let take = width - used;
            let truncated: String = span.content.chars().take(take).collect();
            out.push(Span::styled(truncated, span.style));
            used = width;
            break;
        }
    }
    if used < width {
        out.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(pad_bg),
        ));
    }
    out
}

/// File extension (no dot) of a path's final component, or `""` if none.
fn file_extension(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext)
        .unwrap_or("")
}

/// Push a content segment as spans onto `out`, syntax-highlighting it (per the
/// file `ext`) when `highlight` is set, else a single `fg`-coloured span. Every
/// span gets background `bg`.
fn push_segment(
    out: &mut Vec<Span<'static>>,
    text: &str,
    ext: &str,
    highlight: bool,
    fg: Color,
    bg: Color,
) {
    if highlight {
        for (token, color) in highlight_line(text, ext, fg) {
            out.push(Span::styled(token, Style::default().fg(color).bg(bg)));
        }
    } else {
        out.push(Span::styled(
            text.to_string(),
            Style::default().fg(fg).bg(bg),
        ));
    }
}

/// Apply the reversed (selection-highlight) modifier to every span.
fn reverse_spans(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|s| Span::styled(s.content, s.style.add_modifier(Modifier::REVERSED)))
        .collect()
}

/// A line split into runs of text, each tagged changed (`true`) or unchanged
/// (`false`) by the word-level diff.
type WordSegs = Vec<(String, bool)>;

/// Split a changed (old, new) line pair into segments tagged changed/unchanged,
/// using a character-level common-prefix/suffix heuristic — enough to highlight
/// the edited span within a line without a full intra-line diff.
fn word_diff(old: &str, new: &str) -> (WordSegs, WordSegs) {
    let o: Vec<char> = old.chars().collect();
    let n: Vec<char> = new.chars().collect();
    let min = o.len().min(n.len());

    let mut pre = 0;
    while pre < min && o[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < min - pre && o[o.len() - 1 - suf] == n[n.len() - 1 - suf] {
        suf += 1;
    }

    let split = |chars: &[char]| -> Vec<(String, bool)> {
        let len = chars.len();
        let mut v = Vec::new();
        if pre > 0 {
            v.push((chars[..pre].iter().collect(), false));
        }
        if len - suf > pre {
            v.push((chars[pre..len - suf].iter().collect(), true));
        }
        if suf > 0 {
            v.push((chars[len - suf..].iter().collect(), false));
        }
        if v.is_empty() {
            v.push((String::new(), false));
        }
        v
    };
    (split(&o), split(&n))
}

/// Compute per-selectable-line segment lists for the current file, applying a
/// word-level diff to paired deletion/addition lines within each change block.
/// Unpaired lines (and context) become a single unchanged segment.
fn word_diff_segments(file: &FileDiff) -> Vec<WordSegs> {
    fn flush(
        segs: &mut [WordSegs],
        dels: &mut Vec<(usize, String)>,
        adds: &mut Vec<(usize, String)>,
    ) {
        for i in 0..dels.len().min(adds.len()) {
            let (di, dtext) = &dels[i];
            let (ai, atext) = &adds[i];
            let (dsegs, asegs) = word_diff(dtext, atext);
            segs[*di] = dsegs;
            segs[*ai] = asegs;
        }
        dels.clear();
        adds.clear();
    }

    let mut segs: Vec<WordSegs> = Vec::new();
    for hunk in &file.hunks {
        let (mut dels, mut adds) = (Vec::new(), Vec::new());
        for line in &hunk.lines {
            let idx = segs.len();
            segs.push(vec![(line.content.clone(), false)]);
            match line.origin {
                LineOrigin::Context => flush(&mut segs, &mut dels, &mut adds),
                LineOrigin::Deletion => dels.push((idx, line.content.clone())),
                LineOrigin::Addition => adds.push((idx, line.content.clone())),
            }
        }
        flush(&mut segs, &mut dels, &mut adds);
    }
    segs
}

/// A row in the side-by-side layout.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SbsRow {
    Header(String),
    /// Old-side and new-side selectable indices (`None` = blank half).
    Cells {
        left: Option<usize>,
        right: Option<usize>,
    },
}

/// Pair a file's diff into side-by-side rows. Context lines occupy both halves;
/// runs of deletions/additions in a change block are zipped left/right, padding
/// the shorter side with blanks. Indices are selectable-line indices.
fn side_by_side_rows(file: &FileDiff) -> Vec<SbsRow> {
    fn flush(rows: &mut Vec<SbsRow>, dels: &mut Vec<usize>, adds: &mut Vec<usize>) {
        for i in 0..dels.len().max(adds.len()) {
            rows.push(SbsRow::Cells {
                left: dels.get(i).copied(),
                right: adds.get(i).copied(),
            });
        }
        dels.clear();
        adds.clear();
    }

    let mut rows = Vec::new();
    let mut sel = 0;
    for hunk in &file.hunks {
        rows.push(SbsRow::Header(format!(
            "@@ -{},{} +{},{} @@ {}",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines, hunk.header
        )));
        let (mut dels, mut adds) = (Vec::new(), Vec::new());
        for line in &hunk.lines {
            match line.origin {
                LineOrigin::Context => {
                    flush(&mut rows, &mut dels, &mut adds);
                    rows.push(SbsRow::Cells {
                        left: Some(sel),
                        right: Some(sel),
                    });
                }
                LineOrigin::Deletion => dels.push(sel),
                LineOrigin::Addition => adds.push(sel),
            }
            sel += 1;
        }
        flush(&mut rows, &mut dels, &mut adds);
    }
    rows
}

/// Render the side-by-side body for the current file: old | new columns with
/// per-side line-number gutter, add/remove fills, word-level highlight, and a
/// diagonal-hatch fill for alignment gaps.
fn review_body_lines_side_by_side(
    state: &DiffReviewState,
    focused: bool,
    pal: &ReviewPalette,
    ext: &str,
    highlight: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let Some(file) = state.current_file() else {
        return Vec::new();
    };
    let lines = state.selectable_lines();
    let segs = word_diff_segments(file);
    let (sel_lo, sel_hi) = state.selection();
    // Two columns separated by " │ ".
    let col = width.saturating_sub(3) / 2;

    let cell = |idx: Option<usize>, is_old: bool| -> Vec<Span<'static>> {
        let Some(i) = idx else {
            // Alignment gap: diagonal hatch fill.
            return vec![Span::styled(
                "╱".repeat(col),
                Style::default().fg(pal.gap_fg),
            )];
        };
        let line = lines[i];
        let (line_bg, gutter_bg, emph_bg) = match line.origin {
            LineOrigin::Addition => (pal.add_bg, pal.add_gutter_bg, pal.add_emph_bg),
            LineOrigin::Deletion => (pal.del_bg, pal.del_gutter_bg, pal.del_emph_bg),
            LineOrigin::Context => (Color::Reset, Color::Reset, Color::Reset),
        };
        let no = if is_old {
            line.old_lineno
        } else {
            line.new_lineno
        };
        let mut spans = vec![Span::styled(
            format!("{} ", lineno_str(no)),
            Style::default().fg(pal.gutter_fg).bg(gutter_bg),
        )];
        for (text, emph) in &segs[i] {
            let bg = if *emph { emph_bg } else { line_bg };
            push_segment(&mut spans, text, ext, highlight, pal.text, bg);
        }
        let spans = fit_spans(spans, col, line_bg);
        if focused && i >= sel_lo && i <= sel_hi {
            reverse_spans(spans)
        } else {
            spans
        }
    };

    side_by_side_rows(file)
        .into_iter()
        .map(|row| match row {
            SbsRow::Header(_) => {
                // Re-derive the hunk for a full-width styled header.
                Line::from(Span::styled(
                    row_header_text(&row),
                    Style::default().fg(pal.hunk_header),
                ))
            }
            SbsRow::Cells { left, right } => {
                let mut spans = cell(left, true);
                spans.push(Span::styled(" │ ", Style::default().fg(pal.gutter_fg)));
                spans.extend(cell(right, false));
                Line::from(spans)
            }
        })
        .collect()
}

/// The header text for a [`SbsRow::Header`] (empty for non-header rows).
fn row_header_text(row: &SbsRow) -> String {
    match row {
        SbsRow::Header(h) => h.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::parse_unified_diff;

    fn state_with_two_files() -> DiffReviewState {
        let diff = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 fn main() {
+    let y = 3;
 }
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-b
+B
",
        );
        DiffReviewState::new(
            SessionId::new(),
            "test".to_string(),
            "main".to_string(),
            diff,
            Vec::new(),
        )
    }

    #[test]
    fn next_prev_file_clamps_and_resets() {
        let mut s = state_with_two_files();
        s.cursor = 1;
        s.prev_file();
        assert_eq!(s.selected_file, 0);
        s.next_file();
        assert_eq!(s.selected_file, 1);
        assert_eq!(s.cursor, 0, "cursor resets on file change");
        s.next_file();
        assert_eq!(s.selected_file, 1, "clamps at last file");
    }

    #[test]
    fn move_cursor_clamps() {
        let mut s = state_with_two_files();
        s.focus = ReviewFocus::Body;
        // a.rs has 3 selectable lines (context, addition, context).
        assert_eq!(s.selectable_count(), 3);
        for _ in 0..10 {
            s.move_cursor(true);
        }
        assert_eq!(s.cursor, 2);
        s.move_cursor(false);
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn visual_selection_range_normalises() {
        let mut s = state_with_two_files();
        s.cursor = 2;
        s.toggle_visual(); // anchor at 2
        s.cursor = 0; // active end above anchor
        assert_eq!(s.selection(), (0, 2));
        s.toggle_visual(); // cancel
        assert_eq!(s.selection(), (0, 0));
    }

    #[test]
    fn build_draft_picks_new_side_and_captures_snippet() {
        let mut s = state_with_two_files();
        // Select the inserted line (selectable index 1: "+    let y = 3;").
        s.cursor = 1;
        let draft = s.build_draft((1, 1), "extract helper".to_string()).unwrap();
        assert_eq!(draft.file, "a.rs");
        assert_eq!(draft.side, AnnotationSide::New);
        assert_eq!(draft.line_range, (2, 2));
        assert_eq!(draft.snippet, "    let y = 3;");
        assert_eq!(draft.comment, "extract helper");
    }

    #[test]
    fn build_draft_pure_deletion_uses_old_side() {
        let mut s = state_with_two_files();
        s.selected_file = 1; // b.rs: -b / +B
        // selectable index 0 is the deletion "-b".
        let draft = s.build_draft((0, 0), "why?".to_string()).unwrap();
        assert_eq!(draft.side, AnnotationSide::Old);
        assert_eq!(draft.snippet, "b");
    }

    #[test]
    fn annotation_at_cursor_matches_covering_range() {
        let mut s = state_with_two_files();
        s.annotations.push(Annotation::new(
            "a.rs",
            AnnotationSide::New,
            (2, 2),
            "    let y = 3;",
            "note",
        ));
        s.cursor = 1; // the inserted line (new lineno 2)
        assert!(s.annotation_at_cursor().is_some());
        s.cursor = 0; // context line "fn main() {" (new lineno 1) — not covered
        assert!(s.annotation_at_cursor().is_none());
    }

    #[test]
    fn toggle_focus_flips() {
        let mut s = state_with_two_files();
        assert_eq!(s.focus, ReviewFocus::FileList);
        s.toggle_focus();
        assert_eq!(s.focus, ReviewFocus::Body);
    }

    // --- file tree ---

    fn file(path: &str) -> FileDiff {
        FileDiff {
            old_path: path.to_string(),
            new_path: path.to_string(),
            status: FileStatus::Modified,
            added: 1,
            removed: 0,
            hunks: Vec::new(),
        }
    }

    #[test]
    fn tree_compresses_single_child_dir_chains() {
        let files = vec![
            file("common/src/redux/middleware/a.ts"),
            file("common/src/redux/middleware/b.ts"),
            file("notes/src/app/Runner.ts"),
        ];
        let tree = build_file_tree(&files);
        assert_eq!(tree.len(), 2);
        // Single-child dir chain with two file leaves collapses to one node.
        assert_eq!(tree[0].name, "common/src/redux/middleware");
        assert_eq!(tree[0].children.len(), 2);
        // Chain stops at the directory holding the single file leaf.
        assert_eq!(tree[1].name, "notes/src/app");
        assert_eq!(tree[1].children.len(), 1);
        assert_eq!(tree[1].children[0].name, "Runner.ts");
        assert_eq!(tree[1].children[0].file_index, Some(2));
    }

    #[test]
    fn flatten_respects_collapse() {
        let tree = build_file_tree(&[file("dir/a.ts"), file("dir/b.ts")]);
        let mut collapsed = HashSet::new();
        let mut rows = Vec::new();
        flatten_tree(&tree, 0, &collapsed, &mut rows);
        assert_eq!(rows.len(), 3); // dir + two files

        collapsed.insert("dir".to_string());
        rows.clear();
        flatten_tree(&tree, 0, &collapsed, &mut rows);
        assert_eq!(rows.len(), 1); // collapsed dir hides its files
    }

    #[test]
    fn tree_move_updates_body_and_activate_collapses() {
        let diff = ParsedDiff {
            files: vec![file("dir/a.ts"), file("dir/b.ts")],
        };
        let mut s = DiffReviewState::new(
            SessionId::new(),
            "t".to_string(),
            "main".to_string(),
            diff,
            Vec::new(),
        );
        // rows: Dir(dir) @0, File a @1, File b @2. Body starts on first file.
        assert_eq!(s.selected_file, 0);
        s.tree_move(true); // onto file a
        s.tree_move(true); // onto file b
        assert_eq!(s.selected_file, 1);
        // Back to the dir row and collapse it.
        s.tree_move(false);
        s.tree_move(false);
        assert_eq!(s.tree_cursor, 0);
        s.tree_activate();
        assert_eq!(s.visible_rows().len(), 1);
        // Expanding restores the files.
        s.tree_activate();
        assert_eq!(s.visible_rows().len(), 3);
    }

    #[test]
    fn selectable_at_body_row_skips_header() {
        let s = state_with_two_files();
        // a.rs body rows: 0 header, 1..=3 the three diff lines.
        assert_eq!(s.selectable_at_body_row(0), None);
        assert_eq!(s.selectable_at_body_row(1), Some(0));
        assert_eq!(s.selectable_at_body_row(3), Some(2));
        assert_eq!(s.selectable_at_body_row(4), None);
    }

    #[test]
    fn click_and_drag_select_a_range() {
        let mut s = state_with_two_files();
        let body = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        // Click the first diff line (body row 1).
        s.click_at(5, 1, body);
        assert_eq!(s.focus, ReviewFocus::Body);
        assert_eq!(s.cursor, 0);
        assert!(s.visual_anchor.is_none());
        // Drag down to body row 3 (third diff line) → selection 0..=2.
        s.drag_at(5, 3, body);
        assert_eq!(s.selection(), (0, 2));
        // Clicking outside the body rect leaves the cursor untouched.
        s.click_at(5, 50, body);
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn file_extension_handles_paths_and_dotfiles() {
        assert_eq!(file_extension("src/git/diff.rs"), "rs");
        assert_eq!(file_extension("Cargo.toml"), "toml");
        assert_eq!(file_extension("README.md"), "md");
        assert_eq!(file_extension("Makefile"), "");
        assert_eq!(file_extension("dir.with.dot/Justfile"), "");
    }

    #[test]
    fn word_diff_marks_only_the_changed_span() {
        let (old, new) = word_diff("let y = 2;", "let y = 3;");
        assert_eq!(
            old,
            vec![
                ("let y = ".to_string(), false),
                ("2".to_string(), true),
                (";".to_string(), false),
            ]
        );
        assert_eq!(
            new,
            vec![
                ("let y = ".to_string(), false),
                ("3".to_string(), true),
                (";".to_string(), false),
            ]
        );
    }

    #[test]
    fn word_diff_identical_lines_have_no_change() {
        let (old, new) = word_diff("same", "same");
        assert_eq!(old, vec![("same".to_string(), false)]);
        assert_eq!(new, vec![("same".to_string(), false)]);
    }

    #[test]
    fn word_diff_segments_pairs_replace_block() {
        let diff = parse_unified_diff(
            "\
diff --git a/x.rs b/x.rs
--- a/x.rs
+++ b/x.rs
@@ -1,2 +1,2 @@
 fn f() {
-let y = 2;
+let y = 3;
",
        );
        let segs = word_diff_segments(&diff.files[0]);
        // selectable index 0 = context (unchanged), 1 = deletion, 2 = addition.
        assert_eq!(segs[0], vec![("fn f() {".to_string(), false)]);
        assert!(segs[1].iter().any(|(t, emph)| *emph && t == "2"));
        assert!(segs[2].iter().any(|(t, emph)| *emph && t == "3"));
    }

    #[test]
    fn side_by_side_pairs_change_blocks() {
        let s = state_with_two_files();
        let rows = side_by_side_rows(s.current_file().unwrap());
        assert!(matches!(rows[0], SbsRow::Header(_)));
        // context(0,0), then the lone addition paired with a blank left,
        // then context(2,2).
        assert_eq!(
            &rows[1..],
            &[
                SbsRow::Cells {
                    left: Some(0),
                    right: Some(0)
                },
                SbsRow::Cells {
                    left: None,
                    right: Some(1)
                },
                SbsRow::Cells {
                    left: Some(2),
                    right: Some(2)
                },
            ]
        );
    }

    #[test]
    fn side_by_side_zips_deletion_and_addition() {
        let mut s = state_with_two_files();
        s.selected_file = 1; // b.rs: -b / +B
        let rows = side_by_side_rows(s.current_file().unwrap());
        assert_eq!(
            rows[1],
            SbsRow::Cells {
                left: Some(0),
                right: Some(1)
            }
        );
    }

    #[test]
    fn begin_comment_opens_box_for_selection() {
        let mut s = state_with_two_files();
        s.focus = ReviewFocus::Body;
        s.cursor = 2;
        s.toggle_visual(); // anchor at 2
        s.cursor = 0;
        assert!(s.begin_comment());
        let draft = s.comment.as_ref().unwrap();
        assert_eq!(draft.range, (0, 2));
        // A second call while a comment is open is a no-op.
        assert!(!s.begin_comment());
    }

    #[test]
    fn toggle_layout_flips() {
        let mut s = state_with_two_files();
        assert_eq!(s.layout, ReviewLayout::Inline);
        s.toggle_layout();
        assert_eq!(s.layout, ReviewLayout::SideBySide);
    }

    #[test]
    fn wheel_scrolls_within_bounds() {
        let mut s = state_with_two_files();
        // a.rs total body rows = 1 header + 3 lines = 4 → max scroll 3.
        for _ in 0..10 {
            s.wheel(true);
        }
        assert_eq!(s.scroll, 3);
        s.wheel(false);
        assert_eq!(s.scroll, 2);
    }
}
