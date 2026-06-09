//! Full-screen review-diff-and-comment view.
//!
//! Presentation only: [`DiffReviewState`] holds what's on screen and is opened
//! via `CommanderService::open_review`. The view is hosted as a maximised
//! modal (`Modal::ReviewDiff`); all diff composition, parsing, and comment
//! logic lives in the library.

use super::*;
use crossterm::event::KeyEvent;

use crate::api::NewComment;
use crate::comment::{Comment, CommentSide, CommentStatus};
use crate::git::{DiffLine, FileDiff, FileStatus, Hunk, LineOrigin, ParsedDiff};
use crate::tui::syntax_highlight::highlight_line;
use crate::tui::theme::{ColorMode, ReviewPalette};
use std::cell::{Ref, RefCell};

/// Gutter / badge / box marker for a staged comment. An asterisk is the
/// conventional note/comment marker and stays crisp at one cell.
const COMMENT_MARKER: char = '*';
/// Marker for a drifted comment (its snippet could no longer be located).
const DRIFT_MARKER: char = '⚠';

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
    pub comments: Vec<Comment>,
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
    /// Comment ids whose inline box is collapsed (absent = expanded).
    collapsed_comments: HashSet<uuid::Uuid>,
    /// Memoized word-diff segments for the current file (`selected_file`, segs).
    /// Recomputed only when the body file changes — the LCS pass is O(file) and
    /// would otherwise re-run on every render frame. See [`Self::word_segments`].
    seg_cache: RefCell<Option<(usize, Vec<WordSegs>)>>,
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
        comments: Vec<Comment>,
    ) -> Self {
        let file_tree = build_file_tree(&diff.files);
        // Body starts on the first file in tree order so it shows something.
        let selected_file = first_file_index(&file_tree).unwrap_or(0);
        Self {
            session_id,
            title,
            base,
            diff,
            comments,
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
            collapsed_comments: HashSet::new(),
            seg_cache: RefCell::new(None),
        }
    }

    /// Word-diff segments for the current file, memoized by `selected_file`.
    /// Rebuilt only when the body file changes, so scrolling within a file
    /// reuses the cached LCS result rather than recomputing it each frame.
    fn word_segments(&self) -> Ref<'_, Vec<WordSegs>> {
        let stale = self
            .seg_cache
            .borrow()
            .as_ref()
            .map(|(file, _)| *file != self.selected_file)
            .unwrap_or(true);
        if stale {
            let segs = self
                .current_file()
                .map(word_diff_segments)
                .unwrap_or_default();
            *self.seg_cache.borrow_mut() = Some((self.selected_file, segs));
        }
        Ref::map(self.seg_cache.borrow(), |c| &c.as_ref().unwrap().1)
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

    /// Number of not-yet-applied comments anchored to `file`.
    fn comment_count(&self, file: &str) -> usize {
        self.comments
            .iter()
            .filter(|a| a.status != CommentStatus::Applied && a.file == file)
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

    /// Scroll the diff body by a page (lazygit-style PgUp/PgDn). Independent of
    /// focus, so paging the diff works while the file list is focused.
    fn page_body(&mut self, down: bool) {
        let max = self.total_body_rows().saturating_sub(1) as u16;
        let page = BODY_VIEWPORT as u16;
        self.scroll = if down {
            (self.scroll + page).min(max)
        } else {
            self.scroll.saturating_sub(page)
        };
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

    /// Left-click at a screen position. Clicking a comment box folds or unfolds
    /// it; clicking a diff line focuses the body and moves the cursor there
    /// (clearing any selection).
    pub fn click_at(&mut self, col: u16, row: u16, body: Rect) {
        let Some(body_row) = self.body_row_at(col, row, body) else {
            return;
        };
        if let Some(id) = self.comment_box_at_body_row(body_row, body.width as usize) {
            self.toggle_comment_collapsed(id);
            return;
        }
        self.place_cursor_at_row(body_row);
    }

    /// Focus the body and move the cursor to the diff line at `body_row`,
    /// clearing any active selection. Inline only: in side-by-side the row
    /// structure differs, so this just focuses the body.
    fn place_cursor_at_row(&mut self, body_row: usize) {
        self.focus = ReviewFocus::Body;
        self.visual_anchor = None;
        if self.layout == ReviewLayout::Inline
            && let Some(idx) = self.selectable_at_body_row(body_row)
        {
            self.cursor = idx;
        }
    }

    /// Map a screen position in the file-list pane to a visible tree-row index
    /// (accounting for the tree's scroll offset).
    fn file_row_at(&self, col: u16, row: u16, rect: Rect) -> Option<usize> {
        let inside = col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height;
        inside.then(|| (row - rect.y) as usize + self.tree_scroll as usize)
    }

    /// Left-click in the file-list pane: focus it and move the tree cursor to
    /// the clicked row. A file row is shown in the body; a directory row is
    /// expanded/collapsed (the mouse equivalent of Enter).
    pub fn click_file_list_at(&mut self, col: u16, row: u16, rect: Rect) {
        let Some(idx) = self.file_row_at(col, row, rect) else {
            return;
        };
        let rows = self.visible_rows();
        let file_index = match rows.get(idx) {
            Some(TreeRow::File { index, .. }) => Some(*index),
            Some(TreeRow::Dir { .. }) => None,
            None => return,
        };
        self.focus = ReviewFocus::FileList;
        self.tree_cursor = idx;
        match file_index {
            Some(index) => self.set_body_file(index),
            None => self.tree_activate(),
        }
    }

    /// Right-click at a screen position: open the comment box. With no active
    /// selection, first move the cursor to the clicked line so a bare
    /// right-click comments on the line under the pointer; an in-progress
    /// drag-selection is preserved and commented on as-is.
    pub fn right_click_comment(&mut self, col: u16, row: u16, body: Rect) -> bool {
        // Position the cursor on the clicked line (not via `click_at`, which
        // would toggle a comment box rather than comment on it).
        if self.visual_anchor.is_none()
            && let Some(body_row) = self.body_row_at(col, row, body)
        {
            self.place_cursor_at_row(body_row);
        }
        self.begin_comment()
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

    /// Selectable lines for `range`, clamped to the available lines.
    fn selected_lines(&self, range: (usize, usize)) -> Vec<&DiffLine> {
        let lines = self.selectable_lines();
        let (lo, hi) = range;
        lines
            .get(lo..=hi.min(lines.len().saturating_sub(1)))
            .map(<[_]>::to_vec)
            .unwrap_or_default()
    }

    /// Pick the comment side for a selected line slice (New unless the
    /// selection is purely deletions) and collect each contributing line's
    /// gutter line number + content on that side. Shared by `build_draft`
    /// (snippet/anchor) and the comment-box title so they can't drift apart.
    fn side_and_lines(sel: &[&DiffLine]) -> (CommentSide, Vec<(usize, String)>) {
        let any_new = sel.iter().any(|l| l.new_lineno.is_some());
        let side = if any_new {
            CommentSide::New
        } else {
            CommentSide::Old
        };
        let collected = sel
            .iter()
            .filter_map(|l| {
                let n = match side {
                    CommentSide::New => l.new_lineno,
                    CommentSide::Old => l.old_lineno,
                }?;
                Some((n, l.content.clone()))
            })
            .collect();
        (side, collected)
    }

    /// Resolve a selectable-line range to the gutter line numbers it covers on
    /// its comment side — what the user sees, not the raw selectable index.
    /// Returns `None` if the range contributes no numbered lines.
    fn resolved_line_range(&self, range: (usize, usize)) -> Option<(usize, usize)> {
        let sel = self.selected_lines(range);
        let (_, collected) = Self::side_and_lines(&sel);
        let nums = collected.iter().map(|(n, _)| *n);
        Some((nums.clone().min()?, nums.max()?))
    }

    /// Build an comment draft from a selectable-line range plus comment
    /// text. Picks the New side unless the selection is purely deletions, and
    /// captures the snippet/line range from that side's lines only (so it
    /// re-anchors cleanly).
    fn build_draft(&self, range: (usize, usize), comment: String) -> Option<NewComment> {
        let file = self.current_file()?;
        let sel = self.selected_lines(range);
        if sel.is_empty() {
            return None;
        }
        let (side, collected) = Self::side_and_lines(&sel);
        if collected.is_empty() {
            return None;
        }

        let nums = collected.iter().map(|(n, _)| *n);
        Some(NewComment {
            file: file.display_path().to_string(),
            side,
            line_range: (nums.clone().min().unwrap(), nums.max().unwrap()),
            snippet: collected
                .iter()
                .map(|(_, c)| c.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            comment,
        })
    }

    /// Id of a not-yet-applied comment covering the cursor line, if any.
    fn comment_at_cursor(&self) -> Option<uuid::Uuid> {
        let file = self.current_file()?;
        let line = *self.selectable_lines().get(self.cursor)?;
        self.comments
            .iter()
            .filter(|a| a.status != CommentStatus::Applied && a.file == file.display_path())
            .find(|a| {
                let lineno = match a.side {
                    CommentSide::New => line.new_lineno,
                    CommentSide::Old => line.old_lineno,
                };
                lineno.is_some_and(|n| a.line_range.0 <= n && n <= a.line_range.1)
            })
            .map(|a| a.id)
    }

    /// Whether selectable line `idx` is covered by a non-applied comment,
    /// and whether any such comment is drifted.
    fn comment_marker(&self, idx: usize) -> Option<bool> {
        let file = self.current_file()?;
        let line = *self.selectable_lines().get(idx)?;
        let mut drifted = false;
        let mut found = false;
        for a in self
            .comments
            .iter()
            .filter(|a| a.status != CommentStatus::Applied && a.file == file.display_path())
        {
            let lineno = match a.side {
                CommentSide::New => line.new_lineno,
                CommentSide::Old => line.old_lineno,
            };
            if lineno.is_some_and(|n| a.line_range.0 <= n && n <= a.line_range.1) {
                found = true;
                drifted |= a.status == CommentStatus::Drifted;
            }
        }
        found.then_some(drifted)
    }

    /// Whether comment `id`'s inline box is collapsed.
    fn is_comment_collapsed(&self, id: uuid::Uuid) -> bool {
        self.collapsed_comments.contains(&id)
    }

    /// Fold an expanded comment box, or unfold a collapsed one.
    fn toggle_comment_collapsed(&mut self, id: uuid::Uuid) {
        if !self.collapsed_comments.remove(&id) {
            self.collapsed_comments.insert(id);
        }
    }

    /// If body row `body_row` falls within a rendered comment box, the id of
    /// that comment. Walks the body in the same row order as the renderer
    /// (hunk header, diff line, then any comment boxes anchored to it), so the
    /// interleaved boxes are accounted for. `width` is the body content width
    /// (used to size wrapped boxes), matching the value passed to the renderer.
    fn comment_box_at_body_row(&self, body_row: usize, width: usize) -> Option<uuid::Uuid> {
        let file = self.current_file()?;
        let anchors = self.comment_anchors();
        let mut row = 0usize;
        // Test each comment box anchored after `sel`, advancing `row` past it;
        // returns the comment id when `body_row` lands inside the box.
        let box_hit = |row: &mut usize, sel: usize| -> Option<uuid::Uuid> {
            for ann in anchors.get(&sel)?.iter() {
                let h = comment_box_height(ann, self.is_comment_collapsed(ann.id), width);
                if (*row..*row + h).contains(&body_row) {
                    return Some(ann.id);
                }
                *row += h;
            }
            None
        };

        match self.layout {
            ReviewLayout::Inline => {
                let mut sel = 0usize;
                for hunk in &file.hunks {
                    row += 1; // hunk header
                    for _ in &hunk.lines {
                        row += 1; // the diff line itself
                        if let Some(id) = box_hit(&mut row, sel) {
                            return Some(id);
                        }
                        sel += 1;
                    }
                }
            }
            ReviewLayout::SideBySide => {
                for sbs in side_by_side_rows(file) {
                    row += 1; // header or paired cells
                    if let SbsRow::Cells { left, right } = sbs {
                        // Context rows have left == right; de-dup so a box isn't
                        // counted twice (mirrors the renderer).
                        let sels: Vec<usize> = match (left, right) {
                            (Some(l), Some(r)) if l == r => vec![l],
                            (l, r) => l.into_iter().chain(r).collect(),
                        };
                        for sel in sels {
                            if let Some(id) = box_hit(&mut row, sel) {
                                return Some(id);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Toggle the inline box (expanded/collapsed) of the comment covering
    /// the cursor line, if any.
    fn toggle_comment_fold(&mut self) {
        if let Some(id) = self.comment_at_cursor()
            && !self.collapsed_comments.remove(&id)
        {
            self.collapsed_comments.insert(id);
        }
    }

    /// Map each selectable-line index to the comments whose box should be
    /// drawn just after it — anchored to the last line of the comment's
    /// range on its side.
    fn comment_anchors(&self) -> std::collections::HashMap<usize, Vec<&Comment>> {
        let mut map: std::collections::HashMap<usize, Vec<&Comment>> =
            std::collections::HashMap::new();
        let Some(file) = self.current_file() else {
            return map;
        };
        let display = file.display_path();
        let lines = self.selectable_lines();
        for ann in self
            .comments
            .iter()
            .filter(|a| a.status != CommentStatus::Applied && a.file == display)
        {
            let end = ann.line_range.1;
            for (i, line) in lines.iter().enumerate() {
                let lineno = match ann.side {
                    CommentSide::New => line.new_lineno,
                    CommentSide::Old => line.old_lineno,
                };
                if lineno == Some(end) {
                    map.entry(i).or_default().push(ann);
                    break;
                }
            }
        }
        map
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
                    snapshot.comments,
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

    /// Reload the session's comments and re-anchor them against the current
    /// diff (used after create/delete/apply, which don't change the diff).
    async fn reload_review_comments(&self, state: &mut DiffReviewState) {
        let mut anns = self
            .service
            .list_comments(&state.session_id)
            .await
            .unwrap_or_default();
        crate::comment::reanchor_comments(&mut anns, &state.diff);
        state.comments = anns;
    }

    /// Handle a key while the review view is open. `state` has been moved out
    /// of `self.ui_state.modal`; it is put back unless the view is closed.
    pub(super) async fn handle_review_key(
        &mut self,
        key: KeyEvent,
        mut state: Box<DiffReviewState>,
    ) {
        use crossterm::event::{KeyCode, KeyModifiers};

        // Comment box captures all input while open.
        if state.comment.is_some() {
            self.handle_review_comment_key(key, &mut state).await;
            self.ui_state.modal = Modal::ReviewDiff(state);
            return;
        }

        // Ctrl+Q closes the view (consistency with the tmux-session shortcut),
        // alongside Esc. The modal was already replaced with None on extraction.
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return;
        }

        match key.code {
            // Esc cancels an in-progress selection first; otherwise closes.
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
            // Page the diff regardless of focus (lazygit: scroll the diff while
            // the file list is focused).
            KeyCode::PageDown => state.page_body(true),
            KeyCode::PageUp => state.page_body(false),
            KeyCode::Char('t') => state.toggle_layout(),
            KeyCode::Char('z') => state.toggle_comment_fold(),
            KeyCode::Char('v') if state.focus == ReviewFocus::Body => state.toggle_visual(),
            // Enter: toggle a directory in the tree, or open the comment box in
            // the body.
            KeyCode::Enter if state.focus == ReviewFocus::FileList => state.tree_activate(),
            KeyCode::Enter if state.focus == ReviewFocus::Body => {
                state.begin_comment();
            }
            KeyCode::Char('d') if state.focus == ReviewFocus::Body => {
                if let Some(id) = state.comment_at_cursor() {
                    if let Err(e) = self.service.delete_comment(&state.session_id, id).await {
                        self.set_review_status(&format!("Delete failed: {e}"));
                    } else {
                        self.reload_review_comments(&mut state).await;
                    }
                } else {
                    self.set_review_status("No comment on this line");
                }
            }
            KeyCode::Char('a') => self.apply_review(&mut state).await,
            _ => {}
        }
        self.ui_state.modal = Modal::ReviewDiff(state);
    }

    async fn handle_review_comment_key(&mut self, key: KeyEvent, state: &mut DiffReviewState) {
        use crossterm::event::{KeyCode, KeyModifiers};
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
                    match self.service.create_comment(&state.session_id, ann).await {
                        Ok(_) => {
                            state.visual_anchor = None;
                            self.reload_review_comments(state).await;
                        }
                        Err(e) => self.set_review_status(&format!("Comment failed: {e}")),
                    }
                }
            }
            KeyCode::Backspace => {
                draft.text.pop();
            }
            // Ignore Ctrl-combos (e.g. Ctrl+Q) so they don't insert a literal
            // character into the comment.
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                draft.text.push(c)
            }
            _ => {}
        }
    }

    /// Apply staged comments and report the outcome.
    async fn apply_review(&mut self, state: &mut DiffReviewState) {
        use crate::comment::ApplyOutcome;
        match self.service.apply_comments(&state.session_id).await {
            Ok(ApplyOutcome::Nothing) => self.set_review_status("No staged comments to apply"),
            Ok(ApplyOutcome::Blocked { drifted }) => self.set_review_status(&format!(
                "{} drifted comment(s) block apply — review or delete them",
                drifted.len()
            )),
            Ok(ApplyOutcome::Applied { count, .. }) => {
                self.reload_review_comments(state).await;
                self.set_review_status(&format!("Sent {count} comment(s) to the agent"));
            }
            Ok(ApplyOutcome::Deferred { count, .. }) => {
                self.reload_review_comments(state).await;
                self.set_review_status(&format!(
                    "{count} comment(s) queued — agent busy or stopped"
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
            .constraints([Constraint::Percentage(20), Constraint::Min(0)])
            .split(rows[0]);

        self.render_review_file_list(frame, cols[0], state);
        self.render_review_body(frame, cols[1], state);

        let hint = if state.comment.is_some() {
            " type comment · Enter save · Esc cancel "
        } else if state.visual_anchor.is_some() {
            " ↑↓ extend · Enter/right-click comment · v/Esc cancel selection "
        } else if state.focus == ReviewFocus::FileList {
            " ↑↓/jk move · Enter expand/collapse · PgUp/Dn scroll diff · [ ] file · Tab to diff · ^Q/Esc close "
        } else {
            " ↑↓/jk move · v select · Enter comment · z fold · d delete · a apply · t layout · ^Q/Esc close "
        };
        // The footer doubles as this view's status bar — styled like the app
        // status bar so it reads as a replacement, not a second bar.
        frame.render_widget(
            Paragraph::new(Line::from(hint)).style(self.theme.status_bar()),
            rows[1],
        );

        if state.comment.is_some() {
            self.render_review_comment_box(frame, cols[1], state);
        }
    }

    fn render_review_file_list(&self, frame: &mut Frame, area: Rect, state: &DiffReviewState) {
        let focused = state.focus == ReviewFocus::FileList;
        let pal = self.theme.review_palette();
        let border = if focused {
            pal.border_focused
        } else {
            pal.border_unfocused
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
                    let spans = vec![Span::styled(
                        format!("{indent}{chevron} {name}"),
                        Style::default().fg(pal.dir_fg).add_modifier(Modifier::BOLD),
                    )];
                    let spans = if on_cursor {
                        select_spans(spans, &pal)
                    } else {
                        spans
                    };
                    Line::from(spans)
                }
                TreeRow::File { depth, index, name } => {
                    let file = &state.diff.files[*index];
                    let indent = "  ".repeat(*depth);
                    let marker = file_status_marker(file.status);
                    let count = state.comment_count(file.display_path());
                    let badge = if count > 0 {
                        format!(" {COMMENT_MARKER}{count}")
                    } else {
                        String::new()
                    };
                    // Only the status letter is coloured; the file name stays
                    // the default foreground.
                    let mut spans = vec![
                        Span::raw(format!("{indent}  ")),
                        Span::styled(
                            marker.to_string(),
                            Style::default().fg(file_status_color(file.status, &pal)),
                        ),
                        Span::raw(format!(" {name}{badge}")),
                    ];
                    if on_cursor {
                        spans = select_spans(spans, &pal);
                    }
                    Line::from(spans)
                }
            };
            lines.push(line);
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
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
        let pal = self.theme.review_palette();
        let border = if focused {
            pal.border_focused
        } else {
            pal.border_unfocused
        };

        let title = match state.current_file() {
            Some(f) => format!(" {} — vs {} ", f.display_path(), state.base),
            None => format!(" review — vs {} ", state.base),
        };

        // Syntax highlighting emits RGB foregrounds, so only apply it on
        // true-color terminals; otherwise fall back to the palette text colour.
        let highlight = self.theme.mode == ColorMode::TrueColor;
        let ext = state
            .current_file()
            .map(|f| file_extension(f.display_path()).to_string())
            .unwrap_or_default();
        let width = area.width.saturating_sub(2) as usize;
        let segs = state.word_segments();
        let lines = match state.layout {
            ReviewLayout::Inline => {
                review_body_lines(state, focused, &pal, &ext, highlight, width, &segs)
            }
            ReviewLayout::SideBySide => {
                review_body_lines_side_by_side(state, focused, &pal, &ext, highlight, width, &segs)
            }
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
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
        // Show the gutter line numbers the selection covers, not the raw
        // selectable-line indices (which count deletions and so drift from the
        // displayed numbers).
        let loc = match state.resolved_line_range(draft.range) {
            Some((lo, hi)) if lo == hi => format!("line {lo}"),
            Some((lo, hi)) => format!("lines {lo}–{hi}"),
            None => "line ?".to_string(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .title(format!(" Comment ({loc}) "));
        frame.render_widget(
            Paragraph::new(Line::from(format!("{}▏", draft.text))).block(block),
            area,
        );
    }
}

/// Inner rects of the (file list, diff body) panes for a given modal `area` —
/// the regions a mouse position maps into. Must mirror the layout in
/// `render_review_modal`.
fn review_inner_rects(area: Rect) -> (Rect, Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(20), Constraint::Min(0)])
        .split(rows[0]);
    // Inset by each pane block's border.
    let inset = Margin {
        vertical: 1,
        horizontal: 1,
    };
    (cols[0].inner(inset), cols[1].inner(inset))
}

/// Inner rect of the diff body pane (see [`review_inner_rects`]).
pub(super) fn review_body_inner_rect(area: Rect) -> Rect {
    review_inner_rects(area).1
}

/// Inner rect of the file-list pane (see [`review_inner_rects`]).
pub(super) fn review_file_list_inner_rect(area: Rect) -> Rect {
    review_inner_rects(area).0
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

/// Colour used for a file row by its change status, from the theme palette.
fn file_status_color(status: FileStatus, pal: &ReviewPalette) -> Color {
    match status {
        FileStatus::Added => pal.add_fg,
        FileStatus::Deleted => pal.del_fg,
        FileStatus::Modified => pal.modified_fg,
        FileStatus::Renamed => pal.renamed_fg,
    }
}

/// Build the inline-rendered body for the current file: hunk headers plus each
/// diff line with a coloured gutter, full-width add/remove background fill,
/// word-level intra-line highlight, and an comment marker. Selected lines
/// (cursor or visual range) are reversed when the body is focused.
fn review_body_lines(
    state: &DiffReviewState,
    focused: bool,
    pal: &ReviewPalette,
    ext: &str,
    highlight: bool,
    width: usize,
    segs: &[WordSegs],
) -> Vec<Line<'static>> {
    let Some(file) = state.current_file() else {
        return Vec::new();
    };
    let (sel_lo, sel_hi) = state.selection();
    let anchors = state.comment_anchors();
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
                    pal.add_fg,
                ),
                LineOrigin::Deletion => (
                    pal.del_bg,
                    pal.del_gutter_bg,
                    pal.del_emph_bg,
                    '-',
                    pal.del_fg,
                ),
                LineOrigin::Context => {
                    (Color::Reset, Color::Reset, Color::Reset, ' ', pal.gutter_fg)
                }
            };
            let ann = match state.comment_marker(idx) {
                Some(true) => DRIFT_MARKER,
                Some(false) => COMMENT_MARKER,
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
                spans = select_spans(spans, pal);
            }
            out.push(Line::from(spans));

            // Inline comment box(es) anchored to this line.
            if let Some(anns) = anchors.get(&idx) {
                for ann in anns {
                    out.extend(comment_box_lines(
                        ann,
                        state.is_comment_collapsed(ann.id),
                        width,
                        pal,
                    ));
                }
            }
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

/// Render an comment as an inline box, visually distinct from the diff.
/// Collapsed → a single rounded header bar with a preview; expanded → the bar
/// plus the wrapped comment and a closing border.
/// Number of rendered rows [`comment_box_lines`] produces for the same inputs,
/// without building the styled lines. Used by the click hit-test to walk the
/// body's row layout. Must track `comment_box_lines`'s structure (guarded by a
/// test).
fn comment_box_height(ann: &Comment, collapsed: bool, width: usize) -> usize {
    const INDENT_LEN: usize = 2; // "  "
    let avail = width.saturating_sub(INDENT_LEN);
    if avail < 8 {
        return 0;
    }
    if collapsed {
        return 1;
    }
    let inner = avail - 2;
    let text_width = inner.saturating_sub(2);
    // Top border + wrapped body lines + bottom border.
    let body: usize = ann
        .comment
        .split('\n')
        .map(|paragraph| wrap_text(paragraph, text_width).len())
        .sum();
    body + 2
}

fn comment_box_lines(
    ann: &Comment,
    collapsed: bool,
    width: usize,
    pal: &ReviewPalette,
) -> Vec<Line<'static>> {
    const INDENT: &str = "  ";
    let avail = width.saturating_sub(INDENT.len());
    if avail < 8 {
        return Vec::new();
    }
    let inner = avail - 2; // text columns between the │ borders
    let drifted = ann.status == CommentStatus::Drifted;
    let border = Style::default().fg(if drifted {
        pal.drift_border
    } else {
        pal.comment_border
    });
    // A plain comment needs no marker inside its own box — the border already
    // reads as a comment (the gutter still carries the `*`). Drifted comments
    // keep the ⚠ so the drift stays obvious.
    let marker = if drifted {
        format!("{DRIFT_MARKER} ")
    } else {
        String::new()
    };
    let chevron = if collapsed { '▸' } else { '▾' };

    if collapsed {
        // A single capped horizontal rule (not box corners) so a folded comment
        // reads as one deliberate line rather than the top half of a box.
        let preview = ann.comment.lines().next().unwrap_or("");
        let header = hrule(&format!("{chevron} {marker}{preview} "), inner);
        return vec![Line::from(Span::styled(
            format!("{INDENT}╶{header}╴"),
            border,
        ))];
    }

    let mut out = Vec::new();
    let header = hrule(&format!("{chevron} {marker}comment "), inner);
    out.push(Line::from(Span::styled(
        format!("{INDENT}╭{header}╮"),
        border,
    )));
    let text_width = inner.saturating_sub(2);
    for paragraph in ann.comment.split('\n') {
        for chunk in wrap_text(paragraph, text_width) {
            let body: String = chunk.chars().take(text_width).collect();
            out.push(Line::from(vec![
                Span::styled(format!("{INDENT}│"), border),
                Span::raw(format!(" {body:<text_width$} ")),
                Span::styled("│".to_string(), border),
            ]));
        }
    }
    out.push(Line::from(Span::styled(
        format!("{INDENT}╰{}╯", "─".repeat(inner)),
        border,
    )));
    out
}

/// Build a horizontal-rule string: `head` followed by `─` padding to exactly
/// `width` chars (truncated if `head` is already too long).
fn hrule(head: &str, width: usize) -> String {
    let head = format!("─ {head}");
    let len = head.chars().count();
    if len >= width {
        head.chars().take(width).collect()
    } else {
        format!("{head}{}", "─".repeat(width - len))
    }
}

/// Word-wrap `s` to `width` columns (falls back to hard cuts via the caller's
/// truncation for over-long words). Always returns at least one line.
fn wrap_text(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Apply the theme's selection highlight to every span (background, and
/// foreground when the theme sets one), matching the session list.
fn select_spans(spans: Vec<Span<'static>>, pal: &ReviewPalette) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|s| {
            let mut style = s.style.bg(pal.selection_bg);
            if let Some(fg) = pal.selection_fg {
                style = style.fg(fg);
            }
            Span::styled(s.content, style)
        })
        .collect()
}

/// A line split into runs of text, each tagged changed (`true`) or unchanged
/// (`false`) by the word-level diff.
type WordSegs = Vec<(String, bool)>;

/// Token class for intra-line diffing: identifier runs and whitespace runs are
/// each coalesced into a single token; every other character stands alone.
#[derive(PartialEq, Eq)]
enum TokClass {
    Word,
    Space,
    Other,
}

fn tok_class(c: char) -> TokClass {
    if c.is_alphanumeric() || c == '_' {
        TokClass::Word
    } else if c.is_whitespace() {
        TokClass::Space
    } else {
        TokClass::Other
    }
}

/// Split a line into tokens: maximal identifier/whitespace runs plus
/// single-character punctuation. The concatenation of the tokens is `s`.
fn tokenize(s: &str) -> Vec<&str> {
    let mut toks = Vec::new();
    let mut iter = s.char_indices().peekable();
    while let Some((start, c)) = iter.next() {
        let class = tok_class(c);
        let mut end = start + c.len_utf8();
        // `Other` chars never coalesce; word/space runs absorb their kind.
        if class != TokClass::Other {
            while let Some(&(j, nc)) = iter.peek() {
                if tok_class(nc) == class {
                    end = j + nc.len_utf8();
                    iter.next();
                } else {
                    break;
                }
            }
        }
        toks.push(&s[start..end]);
    }
    toks
}

/// LCS over two token sequences. Returns, for each side, a `keep` flag per
/// token: `true` where the token is part of the longest common subsequence
/// (unchanged), `false` where it was inserted/deleted (changed).
fn lcs_keep(a: &[&str], b: &[&str]) -> (Vec<bool>, Vec<bool>) {
    let (n, m) = (a.len(), b.len());
    // dp[i][j] = LCS length of a[i..] and b[j..].
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut a_keep = vec![false; n];
    let mut b_keep = vec![false; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            a_keep[i] = true;
            b_keep[j] = true;
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (a_keep, b_keep)
}

/// Coalesce tokens into runs of equal changed/unchanged flag.
fn coalesce(toks: &[&str], keep: &[bool]) -> WordSegs {
    let mut segs: WordSegs = Vec::new();
    for (tok, kept) in toks.iter().zip(keep) {
        let changed = !kept;
        match segs.last_mut() {
            Some(last) if last.1 == changed => last.0.push_str(tok),
            _ => segs.push((tok.to_string(), changed)),
        }
    }
    if segs.is_empty() {
        segs.push((String::new(), false));
    }
    segs
}

/// Minimum fraction of the longer line that the two paired lines must share
/// for intra-line emphasis to be shown. Below this they're treated as a
/// wholesale replacement (solid line colour, no word highlight), matching how
/// GitHub suppresses highlighting of coincidental punctuation matches between
/// unrelated lines.
const WORD_DIFF_SIMILARITY: f32 = 0.5;

/// Split a changed (old, new) line pair into segments tagged changed/unchanged,
/// using a token-level diff (identifier/whitespace runs + single punctuation,
/// matched by longest common subsequence). This emphasises only the genuinely
/// different tokens, even when changes are separated by unchanged text.
///
/// When the two lines share too little to read as an edit of one another
/// (positionally paired but unrelated lines from a multi-line replace block),
/// the LCS matches only incidental punctuation, leaving noisy unchanged islands
/// in a sea of highlight. In that case emphasis is suppressed entirely and each
/// line is returned as a single unchanged span — solid red/green, like GitHub.
fn word_diff(old: &str, new: &str) -> (WordSegs, WordSegs) {
    let o = tokenize(old);
    let n = tokenize(new);
    let (o_keep, n_keep) = lcs_keep(&o, &n);

    // Characters shared by the LCS (matched tokens are byte-identical on both
    // sides, so counting one side suffices) over the longer line's length.
    let shared: usize = o
        .iter()
        .zip(&o_keep)
        .filter(|(_, keep)| **keep)
        .map(|(tok, _)| tok.chars().count())
        .sum();
    let longest = old.chars().count().max(new.chars().count());
    if longest == 0 || (shared as f32) / (longest as f32) < WORD_DIFF_SIMILARITY {
        return (
            vec![(old.to_string(), false)],
            vec![(new.to_string(), false)],
        );
    }

    (coalesce(&o, &o_keep), coalesce(&n, &n_keep))
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
    segs: &[WordSegs],
) -> Vec<Line<'static>> {
    let Some(file) = state.current_file() else {
        return Vec::new();
    };
    let lines = state.selectable_lines();
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
            select_spans(spans, pal)
        } else {
            spans
        }
    };

    let anchors = state.comment_anchors();
    let mut out: Vec<Line<'static>> = Vec::new();
    for row in side_by_side_rows(file) {
        match row {
            SbsRow::Header(_) => out.push(Line::from(Span::styled(
                row_header_text(&row),
                Style::default().fg(pal.hunk_header),
            ))),
            SbsRow::Cells { left, right } => {
                let mut spans = cell(left, true);
                spans.push(Span::styled(" │ ", Style::default().fg(pal.gutter_fg)));
                spans.extend(cell(right, false));
                out.push(Line::from(spans));

                // Inline comment box(es) anchored to either side's line.
                // Context rows have left == right, so de-duplicate.
                let sels: Vec<usize> = match (left, right) {
                    (Some(l), Some(r)) if l == r => vec![l],
                    (l, r) => l.into_iter().chain(r).collect(),
                };
                for sel in sels {
                    if let Some(anns) = anchors.get(&sel) {
                        for ann in anns {
                            out.extend(comment_box_lines(
                                ann,
                                state.is_comment_collapsed(ann.id),
                                width,
                                pal,
                            ));
                        }
                    }
                }
            }
        }
    }
    out
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
        assert_eq!(draft.side, CommentSide::New);
        assert_eq!(draft.line_range, (2, 2));
        assert_eq!(draft.snippet, "    let y = 3;");
        assert_eq!(draft.comment, "extract helper");
    }

    #[test]
    fn resolved_line_range_uses_gutter_number_not_selectable_index() {
        // Two deletions precede the addition, so the addition's selectable
        // index (2) is well ahead of its new-side gutter number (1). The
        // comment-box title must show the gutter number.
        let diff = parse_unified_diff(
            "\
diff --git a/x.rs b/x.rs
--- a/x.rs
+++ b/x.rs
@@ -1,3 +1,2 @@
-old a
-old b
+new
 tail
",
        );
        let s = DiffReviewState::new(
            SessionId::new(),
            "t".to_string(),
            "main".to_string(),
            diff,
            Vec::new(),
        );
        // selectable index 2 is the addition; its new-side line number is 1.
        assert_eq!(s.resolved_line_range((2, 2)), Some((1, 1)));
        // And build_draft anchors to the same number, so title and storage agree.
        assert_eq!(
            s.build_draft((2, 2), "x".into()).unwrap().line_range,
            (1, 1)
        );
    }

    #[test]
    fn build_draft_pure_deletion_uses_old_side() {
        let mut s = state_with_two_files();
        s.selected_file = 1; // b.rs: -b / +B
        // selectable index 0 is the deletion "-b".
        let draft = s.build_draft((0, 0), "why?".to_string()).unwrap();
        assert_eq!(draft.side, CommentSide::Old);
        assert_eq!(draft.snippet, "b");
    }

    #[test]
    fn comment_at_cursor_matches_covering_range() {
        let mut s = state_with_two_files();
        s.comments.push(Comment::new(
            "a.rs",
            CommentSide::New,
            (2, 2),
            "    let y = 3;",
            "note",
        ));
        s.cursor = 1; // the inserted line (new lineno 2)
        assert!(s.comment_at_cursor().is_some());
        s.cursor = 0; // context line "fn main() {" (new lineno 1) — not covered
        assert!(s.comment_at_cursor().is_none());
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
    fn click_file_list_selects_file() {
        let mut s = state_with_two_files();
        // Start focused on the body looking at the first file.
        s.focus = ReviewFocus::Body;
        assert_eq!(s.selected_file, 0);
        let rect = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 20,
        };
        // visible tree rows: [a.rs (row 0), b.rs (row 1)]. Click b.rs.
        s.click_file_list_at(5, 1, rect);
        assert_eq!(s.focus, ReviewFocus::FileList);
        assert_eq!(s.tree_cursor, 1);
        assert_eq!(s.selected_file, 1);
        // A click below the last row is a no-op (no panic, file unchanged).
        s.click_file_list_at(5, 10, rect);
        assert_eq!(s.selected_file, 1);
    }

    #[test]
    fn right_click_selects_line_then_opens_comment() {
        let mut s = state_with_two_files();
        let body = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        // No selection yet; cursor at 0. Right-click body row 2 (selectable
        // index 1, the inserted line) should move the cursor there first.
        assert!(s.right_click_comment(5, 2, body));
        assert_eq!(s.cursor, 1);
        let draft = s.comment.as_ref().unwrap();
        assert_eq!(draft.range, (1, 1));
    }

    #[test]
    fn right_click_keeps_active_selection() {
        let mut s = state_with_two_files();
        let body = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        // Drag-select rows 1..=3 (selectable 0..=2), then right-click row 2.
        s.click_at(5, 1, body);
        s.drag_at(5, 3, body);
        assert_eq!(s.selection(), (0, 2));
        assert!(s.right_click_comment(5, 2, body));
        // The multi-line selection is preserved, not collapsed to the click.
        let draft = s.comment.as_ref().unwrap();
        assert_eq!(draft.range, (0, 2));
    }

    #[test]
    fn comment_anchors_map_to_end_line_selidx() {
        let mut s = state_with_two_files();
        // a.rs: ctx(new1,sel0), +let y=3(new2,sel1), ctx(new3,sel2).
        s.comments.push(Comment::new(
            "a.rs",
            CommentSide::New,
            (2, 2),
            "let y = 3;",
            "note",
        ));
        let anchors = s.comment_anchors();
        assert_eq!(anchors.get(&1).map(|v| v.len()), Some(1));
        assert!(!anchors.contains_key(&0));
    }

    #[test]
    fn comment_box_collapsed_single_line_expanded_boxed() {
        let ann = Comment::new(
            "a.rs",
            CommentSide::New,
            (2, 2),
            "let y = 3;",
            "extract helper\nand rename",
        );
        let pal = Theme::truecolor().review_palette();
        assert_eq!(comment_box_lines(&ann, true, 60, &pal).len(), 1);
        // top border + two comment paragraphs + bottom border.
        assert_eq!(comment_box_lines(&ann, false, 60, &pal).len(), 4);
    }

    #[test]
    fn comment_box_header_drops_asterisk_keeps_drift_marker() {
        let pal = Theme::truecolor().review_palette();
        let text_of = |lines: &[Line]| -> String {
            lines[0].spans.iter().map(|s| s.content.as_ref()).collect()
        };

        // A staged comment's box header has no asterisk (the gutter keeps it).
        let mut ann = Comment::new("a.rs", CommentSide::New, (2, 2), "let y = 3;", "note");
        let expanded = text_of(&comment_box_lines(&ann, false, 60, &pal));
        let collapsed = text_of(&comment_box_lines(&ann, true, 60, &pal));
        assert!(expanded.contains("comment"));
        assert!(!expanded.contains(COMMENT_MARKER));
        assert!(!collapsed.contains(COMMENT_MARKER));

        // A drifted comment still surfaces the ⚠ in its box header.
        ann.status = CommentStatus::Drifted;
        let drifted = text_of(&comment_box_lines(&ann, false, 60, &pal));
        assert!(drifted.contains(DRIFT_MARKER));
    }

    #[test]
    fn toggle_comment_fold_flips_state() {
        let mut s = state_with_two_files();
        let ann = Comment::new("a.rs", CommentSide::New, (2, 2), "let y = 3;", "note");
        let id = ann.id;
        s.comments.push(ann);
        s.focus = ReviewFocus::Body;
        s.cursor = 1; // the inserted line, covered by the comment
        assert!(!s.is_comment_collapsed(id));
        s.toggle_comment_fold();
        assert!(s.is_comment_collapsed(id));
        s.toggle_comment_fold();
        assert!(!s.is_comment_collapsed(id));
    }

    #[test]
    fn comment_box_height_matches_rendered() {
        let pal = Theme::truecolor().review_palette();
        for comment in ["short", "one\ntwo\nthree", &"word ".repeat(40)] {
            let ann = Comment::new("a.rs", CommentSide::New, (2, 2), "snip", comment);
            for width in [12usize, 40, 80, 6 /* below the 8-col floor */] {
                for collapsed in [true, false] {
                    assert_eq!(
                        comment_box_height(&ann, collapsed, width),
                        comment_box_lines(&ann, collapsed, width, &pal).len(),
                        "height mismatch (collapsed={collapsed}, width={width})"
                    );
                }
            }
        }
    }

    #[test]
    fn click_on_comment_box_toggles_fold() {
        let mut s = state_with_two_files();
        let ann = Comment::new("a.rs", CommentSide::New, (2, 2), "let y = 3;", "note");
        let id = ann.id;
        s.comments.push(ann);
        let body = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        // Rows: 0 header, 1 ctx, 2 +addition, 3.. comment box. The box is
        // anchored to selectable index 1 (the addition), so it renders after
        // body row 2.
        assert!(!s.is_comment_collapsed(id));
        s.click_at(5, 3, body);
        assert!(
            s.is_comment_collapsed(id),
            "clicking the box should fold it"
        );
        // Collapsed, the box is a single row still at body row 3 — click again
        // to unfold.
        s.click_at(5, 3, body);
        assert!(
            !s.is_comment_collapsed(id),
            "clicking again should unfold it"
        );
    }

    #[test]
    fn click_on_diff_line_does_not_toggle_comment() {
        let mut s = state_with_two_files();
        let ann = Comment::new("a.rs", CommentSide::New, (2, 2), "let y = 3;", "note");
        let id = ann.id;
        s.comments.push(ann);
        let body = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        // Clicking the addition line (body row 2) moves the cursor and leaves
        // the comment expanded.
        s.click_at(5, 2, body);
        assert_eq!(s.cursor, 1);
        assert!(!s.is_comment_collapsed(id));
    }

    #[test]
    fn click_toggles_comment_box_in_side_by_side() {
        let mut s = state_with_two_files();
        s.layout = ReviewLayout::SideBySide;
        let ann = Comment::new("a.rs", CommentSide::New, (2, 2), "let y = 3;", "note");
        let id = ann.id;
        s.comments.push(ann);
        let body = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        // side-by-side rows: 0 header, 1 ctx(1,1), 2 (gap|+addition), then the
        // box anchored to the addition (selectable index 1) at body row 3.
        s.click_at(5, 3, body);
        assert!(s.is_comment_collapsed(id));
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
    fn word_diff_highlights_only_changed_tokens() {
        // Two separated changes on one line (`1`→`9` and `2`→`8`). A
        // char-level common-prefix/suffix heuristic collapses everything
        // between the first and last change into one big emphasised span
        // (highlighting the unchanged `; bar = ` middle). A token-level diff
        // must emphasise *only* the changed numbers.
        let (old, new) = word_diff("foo = 1; bar = 2;", "foo = 9; bar = 8;");

        let old_changed: Vec<&str> = old
            .iter()
            .filter(|(_, e)| *e)
            .map(|(t, _)| t.as_str())
            .collect();
        assert_eq!(old_changed, vec!["1", "2"]);

        let new_changed: Vec<&str> = new
            .iter()
            .filter(|(_, e)| *e)
            .map(|(t, _)| t.as_str())
            .collect();
        assert_eq!(new_changed, vec!["9", "8"]);

        // The shared `; bar = ` between the two changes stays unchanged.
        assert!(old.iter().any(|(t, e)| !e && t.contains("bar")));
        assert!(new.iter().any(|(t, e)| !e && t.contains("bar")));
    }

    #[test]
    fn word_diff_suppresses_emphasis_for_dissimilar_lines() {
        // Two positionally-paired but unrelated lines share only incidental
        // punctuation. Highlighting those coincidental matches leaves dark
        // islands in a sea of emphasis (the GitHub "semantic cleanup" case),
        // so below a similarity threshold there should be no intra-line
        // emphasis at all — the whole line is one unchanged span.
        let (old, new) = word_diff(
            "const result = await remuxBlobRecord(mockRemuxer, inputBlobRecord)",
            "beforeEach(() => {",
        );
        assert!(old.iter().all(|(_, e)| !e), "old: {old:?}");
        assert!(new.iter().all(|(_, e)| !e), "new: {new:?}");
        assert_eq!(
            old.iter().map(|(t, _)| t.as_str()).collect::<String>(),
            "const result = await remuxBlobRecord(mockRemuxer, inputBlobRecord)"
        );
        assert_eq!(
            new.iter().map(|(t, _)| t.as_str()).collect::<String>(),
            "beforeEach(() => {"
        );
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
    fn word_segments_cache_matches_fresh_and_invalidates_on_file_switch() {
        let mut s = state_with_two_files();
        s.selected_file = 0;
        // Cached result must equal a fresh computation for the current file...
        let fresh0 = word_diff_segments(&s.diff.files[0]);
        assert_eq!(*s.word_segments(), fresh0);
        // ...and a second call (warm cache) returns the same data.
        assert_eq!(*s.word_segments(), fresh0);
        // Switching the body file invalidates the memo (keyed on selected_file).
        s.selected_file = 1;
        let fresh1 = word_diff_segments(&s.diff.files[1]);
        assert_eq!(*s.word_segments(), fresh1);
        assert_ne!(fresh0, fresh1, "the two files must differ for a real test");
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
    fn page_body_scrolls_a_page_and_clamps() {
        let mut s = state_with_two_files();
        // Focused on the file list, paging still scrolls the diff body.
        assert_eq!(s.focus, ReviewFocus::FileList);
        s.page_body(true);
        // a.rs body has 4 rows (1 header + 3 lines) → clamps to max scroll 3.
        assert_eq!(s.scroll, 3);
        s.page_body(false);
        assert_eq!(s.scroll, 0);
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
