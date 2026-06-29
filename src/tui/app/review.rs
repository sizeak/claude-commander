//! Full-screen review-diff-and-comment view.
//!
//! Presentation only: [`DiffReviewState`] holds what's on screen and is opened
//! via `CommanderService::open_review`. The view is hosted as a maximised
//! modal (`Modal::ReviewDiff`); all diff composition, parsing, and comment
//! logic lives in the library.

use super::*;
use crossterm::event::KeyEvent;

use crate::api::{DiffSide, NewComment};
use crate::comment::{Comment, CommentSide, CommentStatus};
use crate::git::{DiffLine, FileDiff, FileStatus, Hunk, LineOrigin, ParsedDiff};
use crate::tui::syntax_highlight::{highlight_line, warm_highlight_cache};
use crate::tui::theme::{ColorMode, ReviewPalette};
use rayon::prelude::*;
use std::cell::{Cell, Ref, RefCell};
use tui_input::Input;

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

/// Columns the inline gutter occupies before the code content, summing to 14:
/// left edge bar (1), comment marker (1), old lineno (4), gap (1), new lineno
/// (4), gap (1), sign (1), and a gap (1) after the sign so code doesn't butt
/// against the +/-. Long content soft-wraps within [`inline_content_width`].
const INLINE_GUTTER_COLS: usize = 14;

/// A small right margin kept clear when soft-wrapping, so wrapped text doesn't
/// butt directly against the body border (the background fill still extends to
/// the edge; only the wrap point is pulled in).
const INLINE_WRAP_RIGHT_MARGIN: usize = 2;

/// Columns available for code content on an inline row — i.e. the soft-wrap
/// width: the body width minus the fixed gutter and a small right margin. Used
/// by both the renderer and the row-layout mapping so they wrap identically.
fn inline_content_width(body_width: usize) -> usize {
    body_width.saturating_sub(INLINE_GUTTER_COLS + INLINE_WRAP_RIGHT_MARGIN)
}

/// One physical (rendered) row of the inline body. Long diff lines soft-wrap to
/// several `Line { cont: true }` rows, and comment boxes interleave as `Comment`
/// rows, so this is the single source of truth that the renderer and every
/// row↔line mapping (cursor, scroll, click) derive from — keeping them in lock
/// step regardless of wrapping or interleaved boxes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyRow {
    /// A hunk header (`@@ … @@`).
    Header,
    /// A diff line (`sel` = selectable-line index); `cont` is true for the
    /// soft-wrap continuation rows after the first.
    Line { sel: usize, cont: bool },
    /// A row belonging to an interleaved comment box.
    Comment { id: uuid::Uuid },
    /// A row belonging to the in-progress comment edit box.
    Draft,
}

/// Greedy word-wrap of `text` into rows at most `width` columns wide, returning
/// each row's length in characters. Breaks at whitespace; a single word longer
/// than `width` is hard-split. Every character is preserved (no collapsing) and
/// the lengths sum to `text.chars().count()`, so a styled span stream can be
/// re-sliced by these lengths without losing any content or styling. Always
/// returns at least one row.
fn wrap_row_lens(text: &str, width: usize) -> Vec<usize> {
    let total = text.chars().count();
    if width == 0 || total == 0 {
        return vec![total];
    }
    // Runs of whitespace / non-whitespace, as (length, is_whitespace).
    let mut tokens: Vec<(usize, bool)> = Vec::new();
    for ch in text.chars() {
        let ws = ch.is_whitespace();
        match tokens.last_mut() {
            Some((len, last_ws)) if *last_ws == ws => *len += 1,
            _ => tokens.push((1, ws)),
        }
    }

    let mut lens = Vec::new();
    let mut row = 0usize; // chars on the current row
    for &(mut remaining, is_ws) in &tokens {
        while remaining > 0 {
            if row == width {
                lens.push(row);
                row = 0;
            }
            let space = width - row;
            if remaining <= space {
                row += remaining;
                remaining = 0;
            } else if row > 0 && !is_ws && remaining <= width {
                // A whole word that doesn't fit here but fits on its own row:
                // wrap to a fresh row rather than splitting it.
                lens.push(row);
                row = 0;
            } else {
                // Fills the row exactly (over-long word or whitespace run).
                row += space;
                remaining -= space;
                lens.push(row);
                row = 0;
            }
        }
    }
    if row > 0 || lens.is_empty() {
        lens.push(row);
    }
    lens
}

/// Number of physical rows a diff line's `content` occupies when word-wrapped
/// into `content_width`-wide rows. Defers to [`wrap_row_lens`] so it matches the
/// renderer exactly (guarded by a test).
fn line_wrap_rows(content: &str, content_width: usize) -> usize {
    wrap_row_lens(content, content_width).len()
}

/// A review image's load state, cached on `App` keyed by (display path, side).
/// Lives on `App` (not `DiffReviewState`) because `StatefulProtocol` isn't
/// `Clone` and `DiffReviewState` derives `Clone`.
pub(crate) enum ImageEntry {
    /// Fetch + decode in flight; nothing to draw yet.
    Pending,
    /// Fetch or decode failed; the string is a short reason for display.
    Failed(String),
    /// Decoded and ready: a resize protocol bound to the detected terminal
    /// graphics capability. Boxed — it's far larger than the other variants.
    Ready(Box<ratatui_image::protocol::StatefulProtocol>),
}

/// The side of `file`'s image to display: forced for added/deleted files
/// (which have only one side), else the user's toggle preference.
pub(super) fn shown_image_side(file: &FileDiff, pref: DiffSide) -> DiffSide {
    match file.status {
        FileStatus::Added => DiffSide::New,
        FileStatus::Deleted => DiffSide::Old,
        _ => pref,
    }
}

/// The path of `file` on a given side (differs only for renames).
fn side_path(file: &FileDiff, side: DiffSide) -> &str {
    match side {
        DiffSide::Old => &file.old_path,
        DiffSide::New => &file.new_path,
    }
}

/// An in-progress comment, captured against a selectable-line range.
#[derive(Debug, Clone)]
pub struct CommentDraft {
    /// Editable comment text plus cursor (insert/delete/navigate in place),
    /// backed by `tui-input`.
    pub input: Input,
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
    /// xxh3 hash of the raw diff this view was last built from. Lets the
    /// background refresh skip a rebuild when the working tree is unchanged.
    pub content_hash: u64,
    /// Display paths of files marked reviewed (mirrors the persisted store;
    /// stale marks are pruned by the service before the view opens).
    pub reviewed: HashSet<String>,
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
    /// Which side of a binary image to show. Clamped per file: added files
    /// always show New, deleted always show Old (see [`Self::shown_image_side`]).
    pub image_side: DiffSide,
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
    /// Memoized word-diff segments, one slot per file in `diff.files` order
    /// (`None` until first computed). The LCS pass is O(file) and would
    /// otherwise re-run on every render frame; slots are filled lazily by
    /// [`Self::word_segments`] or eagerly by [`Self::prime_segments`] when the
    /// open-review background task precomputes them all up front.
    seg_cache: RefCell<Vec<Option<Vec<WordSegs>>>>,
    /// Body content width (pane inner width) from the most recent render, so the
    /// keypress-time cursor/scroll math wraps lines the same way the renderer
    /// does. Interior-mutable: the renderer holds `&self`.
    body_width: Cell<usize>,
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
        // One memo slot per file (at least one, so `selected_file` always
        // indexes a slot even for an empty diff).
        let seg_slots = diff.files.len().max(1);
        Self {
            session_id,
            title,
            base,
            diff,
            comments,
            content_hash: 0,
            reviewed: HashSet::new(),
            selected_file,
            scroll: 0,
            focus: ReviewFocus::FileList,
            cursor: 0,
            visual_anchor: None,
            comment: None,
            layout: ReviewLayout::Inline,
            image_side: DiffSide::New,
            file_tree,
            collapsed: HashSet::new(),
            tree_cursor: 0,
            tree_scroll: 0,
            collapsed_comments: HashSet::new(),
            seg_cache: RefCell::new(vec![None; seg_slots]),
            // A reasonable default until the first render reports the real width
            // (keypress math before any render is then close enough to self-heal).
            body_width: Cell::new(120),
        }
    }

    /// Word-diff segments for the current file, memoized per file. The slot is
    /// filled on first access (or already populated by [`Self::prime_segments`]),
    /// so scrolling within a file reuses the cached LCS result rather than
    /// recomputing it each frame.
    fn word_segments(&self) -> Ref<'_, Vec<WordSegs>> {
        let idx = self.selected_file;
        let needs = self
            .seg_cache
            .borrow()
            .get(idx)
            .map(|slot| slot.is_none())
            .unwrap_or(true);
        if needs {
            let segs = self
                .current_file()
                .map(word_diff_segments)
                .unwrap_or_default();
            if let Some(slot) = self.seg_cache.borrow_mut().get_mut(idx) {
                *slot = Some(segs);
            }
        }
        Ref::map(self.seg_cache.borrow(), |c| {
            c.get(idx).and_then(|s| s.as_ref()).unwrap_or(empty_segs())
        })
    }

    /// Install fully-precomputed word-diff segments (one entry per file in
    /// `diff.files` order). Called once after the open-review background task
    /// builds every file's segments off-thread, so the first navigation to each
    /// file is instant. Uses interior mutability so it can run on the freshly
    /// built (immutable) state before it's boxed into the modal.
    pub(super) fn prime_segments(&self, segments: Vec<Vec<WordSegs>>) {
        let mut cache = self.seg_cache.borrow_mut();
        for (i, segs) in segments.into_iter().enumerate() {
            if let Some(slot) = cache.get_mut(i) {
                *slot = Some(segs);
            }
        }
    }

    /// Replace the displayed diff with a freshly composed one (the working
    /// tree changed while the view stayed open), preserving navigation state
    /// where it still makes sense: the body stays on the same file by path, the
    /// cursor and scroll clamp into the new content, collapsed directories and
    /// reviewed marks are kept, and any in-progress visual selection is dropped
    /// (line indices may have moved). Render caches are reset and re-primed from
    /// the precomputed `segments`.
    pub(super) fn refresh_diff(
        &mut self,
        diff: ParsedDiff,
        comments: Vec<Comment>,
        reviewed: HashSet<String>,
        segments: Vec<Vec<WordSegs>>,
        content_hash: u64,
    ) {
        let prev_path = self.current_file().map(|f| f.display_path().to_string());
        self.diff = diff;
        self.comments = comments;
        self.reviewed = reviewed;
        self.content_hash = content_hash;
        self.file_tree = build_file_tree(&self.diff.files);
        // Reset and re-prime the per-file segment cache for the new file set.
        let seg_slots = self.diff.files.len().max(1);
        self.seg_cache = RefCell::new(vec![None; seg_slots]);
        self.prime_segments(segments);
        self.visual_anchor = None;
        // Re-locate the file that was on screen by its path; fall back to the
        // first file when it left the diff.
        self.selected_file = prev_path
            .and_then(|p| self.diff.files.iter().position(|f| f.display_path() == p))
            .or_else(|| first_file_index(&self.file_tree))
            .unwrap_or(0)
            .min(self.diff.files.len().saturating_sub(1));
        // Clamp the cursor into the (possibly shorter) current file, keep it in
        // view, and resync the tree cursor onto the shown file.
        let count = self.selectable_count();
        self.cursor = self.cursor.min(count.saturating_sub(1));
        self.follow_cursor();
        self.sync_tree_cursor_to_file();
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

    /// Flip the preferred binary-image side (before ⇄ after). No-op visually on
    /// added/deleted files, which always show their only side.
    pub(super) fn toggle_image_side(&mut self) {
        self.image_side = match self.image_side {
            DiffSide::Old => DiffSide::New,
            DiffSide::New => DiffSide::Old,
        };
    }

    /// Whether flipping the image side actually does something: the current
    /// file is a *modified* binary image (added/deleted images show only their
    /// one side). Gates both the footer hint and the `o` telemetry, so no-op
    /// presses on text/added/deleted files aren't counted.
    pub(super) fn can_toggle_image_side(&self) -> bool {
        self.current_file().is_some_and(|f| {
            f.status == FileStatus::Modified
                && matches!(
                    f.binary.as_ref().map(|b| &b.kind),
                    Some(crate::git::BinaryKind::Image { .. })
                )
        })
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

    /// Pending comments across every file under directory `dir_path`, so a
    /// collapsed directory still surfaces that its subtree has comments.
    fn dir_comment_count(&self, dir_path: &str) -> usize {
        let prefix = format!("{dir_path}/");
        self.comments
            .iter()
            .filter(|a| a.status != CommentStatus::Applied && a.file.starts_with(&prefix))
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

    /// Whether `path` (a display path) is marked reviewed.
    fn is_reviewed_path(&self, path: &str) -> bool {
        self.reviewed.contains(path)
    }

    /// Set or clear the reviewed mark for `path` in the view's local mirror
    /// (the persisted store is updated by the service).
    fn set_reviewed(&mut self, path: String, on: bool) {
        if on {
            self.reviewed.insert(path);
        } else {
            self.reviewed.remove(&path);
        }
    }

    /// Jump the body to the next unreviewed file after the current one (in
    /// diff order), wrapping around; stays put when every file is reviewed.
    fn advance_to_next_unreviewed(&mut self) {
        let count = self.diff.files.len();
        let next = (1..count)
            .map(|step| (self.selected_file + step) % count)
            .find(|&idx| !self.is_reviewed_path(self.diff.files[idx].display_path()));
        if let Some(idx) = next {
            self.set_body_file(idx);
            self.sync_tree_cursor_to_file();
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

    /// The inline body's physical rows, in render order: a hunk header, then
    /// each diff line (one row, plus continuation rows when its content
    /// soft-wraps past `content_width = body_width - INLINE_GUTTER_COLS`), then
    /// any comment boxes anchored to that line. The single layout used by the
    /// inline renderer and by every row↔line mapping below.
    fn inline_physical_rows(&self) -> Vec<BodyRow> {
        let mut rows = Vec::new();
        let Some(file) = self.current_file() else {
            return rows;
        };
        let body_width = self.body_width.get();
        let content_width = inline_content_width(body_width);
        let anchors = self.comment_anchors();
        let draft_anchor = self.draft_anchor();
        let mut sel = 0;
        for hunk in &file.hunks {
            rows.push(BodyRow::Header);
            for line in &hunk.lines {
                let h = line_wrap_rows(&line.content, content_width);
                for c in 0..h {
                    rows.push(BodyRow::Line { sel, cont: c > 0 });
                }
                if let Some(anns) = anchors.get(&sel) {
                    for ann in anns {
                        let bh =
                            comment_box_height(ann, self.is_comment_collapsed(ann.id), body_width);
                        for _ in 0..bh {
                            rows.push(BodyRow::Comment { id: ann.id });
                        }
                    }
                }
                if draft_anchor == Some(sel)
                    && let Some(draft) = self.comment.as_ref()
                {
                    for _ in 0..comment_draft_box_height(
                        &super::input_with_caret(&draft.input),
                        body_width,
                    ) {
                        rows.push(BodyRow::Draft);
                    }
                }
                sel += 1;
            }
        }
        rows
    }

    /// The selectable-line index the in-progress comment edit box anchors to
    /// (the last line of its range), or `None` when no comment is being edited.
    /// The edit box renders just after this line, where the saved comment will.
    fn draft_anchor(&self) -> Option<usize> {
        self.comment.as_ref().map(|d| d.range.1)
    }

    /// Adjust `scroll` so the in-progress comment edit box stays visible (inline
    /// only). Keeps the box's last row in view as it grows with typed text, and
    /// pulls the top into view if the box starts above the viewport.
    fn follow_draft(&mut self) {
        if self.layout != ReviewLayout::Inline || self.comment.is_none() {
            return;
        }
        let rows = self.inline_physical_rows();
        let first = rows.iter().position(|r| matches!(r, BodyRow::Draft));
        let last = rows.iter().rposition(|r| matches!(r, BodyRow::Draft));
        if let (Some(first), Some(last)) = (first, last) {
            let bottom = self.scroll as usize + BODY_VIEWPORT;
            if last >= bottom {
                self.scroll = (last + 1).saturating_sub(BODY_VIEWPORT) as u16;
            }
            if (first as u16) < self.scroll {
                self.scroll = first as u16;
            }
        }
    }

    /// Physical body row of selectable line `idx`'s first (non-continuation)
    /// row. In side-by-side the row structure differs, so a simpler linear walk
    /// is used (it never wraps).
    fn body_row_of(&self, idx: usize) -> usize {
        if self.layout == ReviewLayout::Inline {
            return self
                .inline_physical_rows()
                .iter()
                .position(|r| matches!(r, BodyRow::Line { sel, cont: false } if *sel == idx))
                .unwrap_or(0);
        }
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
            input: Input::default(),
            range: self.selection(),
        });
        self.follow_draft();
        true
    }

    /// Append pasted clipboard text to the open comment draft. Newlines are
    /// kept — bracketed paste delivers them as text rather than Enter key
    /// events, so a multi-line paste can't accidentally submit, and
    /// `compose_markdown` passes them to the agent verbatim. Carriage
    /// returns from CRLF clipboards are dropped. Returns `false` (no-op)
    /// when no comment is being edited.
    pub fn paste_into_draft(&mut self, text: &str) -> bool {
        let Some(draft) = self.comment.as_mut() else {
            return false;
        };
        // `tui-input` has no bulk insert, so feed chars one at a time at the
        // cursor. CRs from CRLF clipboards are dropped; newlines are kept.
        for c in text.chars().filter(|&c| c != '\r') {
            draft.input.handle(tui_input::InputRequest::InsertChar(c));
        }
        self.follow_draft();
        true
    }

    /// Total physical body rows for the current file (hunk headers, diff lines
    /// incl. soft-wrap continuations, and interleaved comment boxes).
    fn total_body_rows(&self) -> usize {
        if self.layout == ReviewLayout::Inline {
            return self.inline_physical_rows().len();
        }
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

    /// Selectable-line index at body row `body_row` (`None` for a header,
    /// comment-box, or out-of-range row). A click on a soft-wrap continuation
    /// row maps to its diff line. Inline only — the inverse of
    /// [`Self::body_row_of`]; callers gate side-by-side out.
    fn selectable_at_body_row(&self, body_row: usize) -> Option<usize> {
        match self.inline_physical_rows().get(body_row) {
            Some(BodyRow::Line { sel, .. }) => Some(*sel),
            _ => None,
        }
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

    /// Double-click at a screen position: select the single diff line under the
    /// pointer and open its comment box (the mouse equivalent of right-click on
    /// a fresh line). Returns `false` — leaving the caller to fall back to a
    /// plain click — when the row is a header or comment box rather than a
    /// selectable diff line.
    pub fn double_click_comment(&mut self, col: u16, row: u16, body: Rect) -> bool {
        let Some(body_row) = self.body_row_at(col, row, body) else {
            return false;
        };
        if self.selectable_at_body_row(body_row).is_none() {
            return false;
        }
        self.place_cursor_at_row(body_row);
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

    /// Selectable-line index a non-applied comment anchors to in the current
    /// file: the line whose gutter number equals the end of the comment's
    /// range, or — when that line is no longer present in the diff (a drifted
    /// comment whose range fell outside the current hunks) — the file's last
    /// selectable line. Pinning orphans to the last line keeps them visible
    /// and deletable instead of silently dropping them. `None` only when the
    /// file has no selectable lines at all.
    fn comment_anchor_index(&self, ann: &Comment, lines: &[&DiffLine]) -> Option<usize> {
        if lines.is_empty() {
            return None;
        }
        let end = ann.line_range.1;
        let matched = lines.iter().position(|line| {
            let lineno = match ann.side {
                CommentSide::New => line.new_lineno,
                CommentSide::Old => line.old_lineno,
            };
            lineno == Some(end)
        });
        Some(matched.unwrap_or(lines.len() - 1))
    }

    /// Whether non-applied comment `ann` is reachable at selectable index
    /// `idx`: either `idx`'s gutter line falls within the comment's range, or
    /// `idx` is the comment's drift-fallback anchor line. The fallback only
    /// fires for the file's last selectable line, so a drifted comment whose
    /// range left the diff stays selectable (and thus deletable).
    fn comment_touches_index(&self, ann: &Comment, idx: usize, lines: &[&DiffLine]) -> bool {
        if let Some(line) = lines.get(idx) {
            let lineno = match ann.side {
                CommentSide::New => line.new_lineno,
                CommentSide::Old => line.old_lineno,
            };
            if lineno.is_some_and(|n| ann.line_range.0 <= n && n <= ann.line_range.1) {
                return true;
            }
        }
        idx + 1 == lines.len() && self.comment_anchor_index(ann, lines) == Some(idx)
    }

    /// Id of a not-yet-applied comment covering the cursor line, if any.
    fn comment_at_cursor(&self) -> Option<uuid::Uuid> {
        let file = self.current_file()?;
        let display = file.display_path();
        let lines = self.selectable_lines();
        self.comments
            .iter()
            .filter(|a| a.status != CommentStatus::Applied && a.file == display)
            .find(|a| self.comment_touches_index(a, self.cursor, &lines))
            .map(|a| a.id)
    }

    /// Whether selectable line `idx` is covered by a non-applied comment,
    /// and whether any such comment is drifted.
    fn comment_marker(&self, idx: usize) -> Option<bool> {
        let file = self.current_file()?;
        let display = file.display_path();
        let lines = self.selectable_lines();
        let mut drifted = false;
        let mut found = false;
        for a in self
            .comments
            .iter()
            .filter(|a| a.status != CommentStatus::Applied && a.file == display)
        {
            if self.comment_touches_index(a, idx, &lines) {
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
                // Inline shares the renderer's exact physical layout (wrapping
                // and boxes), so a direct row lookup is correct.
                if let Some(BodyRow::Comment { id }) = self.inline_physical_rows().get(body_row) {
                    return Some(*id);
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
                            // Advance past the in-progress edit box (not a
                            // toggle target) so boxes below it stay aligned.
                            if self.draft_anchor() == Some(sel)
                                && let Some(draft) = self.comment.as_ref()
                            {
                                row += comment_draft_box_height(
                                    &super::input_with_caret(&draft.input),
                                    width,
                                );
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
            if let Some(idx) = self.comment_anchor_index(ann, &lines) {
                map.entry(idx).or_default().push(ann);
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
                // Opt-out: build each file's render caches lazily on first
                // navigation instead. Opens instantly; the first view of a large
                // file can be briefly janky.
                if !self.config.precompute_review_caches {
                    let content_hash = snapshot.content_hash;
                    let mut state = DiffReviewState::new(
                        session_id,
                        title,
                        snapshot.base,
                        snapshot.diff,
                        snapshot.comments,
                    );
                    state.content_hash = content_hash;
                    state.reviewed = snapshot.reviewed.into_iter().collect();
                    self.reset_review_images();
                    self.ensure_review_image(&state).await;
                    self.ui_state.modal = Modal::ReviewDiff(Box::new(state));
                    return;
                }
                // Default: precompute every file's render caches (word-diff
                // segments + syntax highlighting) up front on a worker thread
                // while a spinner shows, then swap in the ready-to-render view —
                // trading the open-time wait for instant file switching.
                let file_count = snapshot.diff.files.len();
                self.ui_state.modal = Modal::Loading {
                    title: "Preparing review".to_string(),
                    message: format!(
                        "Highlighting {file_count} file{}…",
                        if file_count == 1 { "" } else { "s" }
                    ),
                    hint: Some(
                        "Disable \"Precompute Review Caches\" in settings to skip this".to_string(),
                    ),
                };
                let highlight = self.theme.mode == ColorMode::TrueColor;
                let text_fg = self.theme.review_palette().text;
                let tx = self.event_loop.sender();
                let base = snapshot.base;
                let diff = snapshot.diff;
                let comments = snapshot.comments;
                let reviewed = snapshot.reviewed;
                let content_hash = snapshot.content_hash;
                tokio::spawn(async move {
                    // The precompute is CPU-bound and synchronous, so keep it off
                    // the async worker pool; hand the diff back out with its
                    // segments rather than cloning it.
                    let (diff, segments) = tokio::task::spawn_blocking(move || {
                        let segments = precompute_review_caches(&diff, highlight, text_fg);
                        (diff, segments)
                    })
                    .await
                    .expect("review precompute task panicked");
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::ReviewPrepared {
                            prepared: Box::new(ReviewPrepared {
                                session_id,
                                title,
                                base,
                                diff,
                                comments,
                                reviewed,
                                segments,
                                content_hash,
                            }),
                        }))
                        .await;
                });
            }
            Err(e) => {
                self.ui_state.modal = Modal::Error {
                    message: format!("Failed to open review: {e}"),
                };
            }
        }
    }

    pub(super) fn set_review_status(&mut self, msg: &str) {
        self.ui_state.status_message =
            Some((msg.to_string(), Instant::now() + Duration::from_secs(3)));
    }

    /// Reload the session's comments and re-anchor them against the current
    /// diff (used after create/delete/apply, which don't change the diff).
    async fn reload_review_comments(&mut self, state: &mut DiffReviewState) {
        let mut anns = self
            .service
            .list_comments(&state.session_id)
            .await
            .unwrap_or_default();
        crate::comment::reanchor_comments(&mut anns, &state.diff);
        // Keep the session-list pending-comment marker in sync without a disk
        // scan: we already have this session's full comment set in hand.
        let pending = anns.iter().any(|a| a.status != CommentStatus::Applied);
        if pending {
            self.ui_state
                .sessions_with_comments
                .insert(state.session_id);
        } else {
            self.ui_state
                .sessions_with_comments
                .remove(&state.session_id);
        }
        state.comments = anns;
    }

    /// Rescan the comment store and refresh the set of sessions with pending
    /// comments (drives the session-list `*` marker). Run at startup to surface
    /// comments left over from a previous run.
    pub(super) async fn refresh_comment_indicators(&mut self) {
        if let Ok(set) = self.service.sessions_with_pending_comments().await {
            self.ui_state.sessions_with_comments = set;
        }
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

        // Re-attaching to the session mirrors the (rebindable) OpenReviewDiff
        // key that switches an attached session to its review diff (`Alt-r` by
        // default), so the pair toggles back and forth. The modal is already
        // None; `handle_select` queues the attach and quits the TUI loop,
        // which `run()` then picks up.
        if crate::config::keybindings::matches_review_toggle(&self.config.keybindings, &key) {
            self.ui_state.selected_session_id = Some(state.session_id);
            self.handle_select().await;
            return;
        }

        // Ctrl-n / Ctrl-p mirror the arrow keys (and j/k) for navigation,
        // matching the convention used by the other list modals.
        let nav_code = review_nav_keycode(key);
        // Record UI-only review features (layout/fold/image/visual/refresh) that
        // don't flow through an instrumented service method. Comment create /
        // delete / apply and reviewed-toggle are recorded at the service layer.
        if let Some(feature) = review_key_feature(nav_code, state.focus) {
            self.service.telemetry().feature(feature);
        }
        match nav_code {
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
            // Flip the before/after side of a binary image (no-op for non-image
            // files and for added/deleted files, which have only one side).
            // Record only when it does something, so no-op presses on text
            // files don't inflate the metric.
            KeyCode::Char('o') => {
                if state.can_toggle_image_side() {
                    self.service.telemetry().feature("review.toggle_image_side");
                }
                state.toggle_image_side();
            }
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
            // Toggle the reviewed mark on the file shown in the body (either
            // focus); marking advances to the next unreviewed file.
            KeyCode::Char('m') => {
                if let Some(file) = state.current_file().cloned() {
                    match self
                        .service
                        .toggle_file_reviewed(&state.session_id, &file)
                        .await
                    {
                        Ok(now_reviewed) => {
                            state.set_reviewed(file.display_path().to_string(), now_reviewed);
                            if now_reviewed {
                                state.advance_to_next_unreviewed();
                            }
                        }
                        Err(e) => self.set_review_status(&format!("Mark failed: {e}")),
                    }
                }
            }
            KeyCode::Char('a') => self.apply_review(&mut state).await,
            // Manually re-compose the diff against the working tree, folding in
            // any edits made since the view opened (e.g. by the agent acting on
            // applied comments). Idle agents trigger this automatically too.
            KeyCode::Char('r') => {
                let (sid, title, prev_hash) =
                    (state.session_id, state.title.clone(), state.content_hash);
                self.spawn_review_refresh(sid, title, prev_hash, true);
            }
            _ => {}
        }
        // A navigation key or side toggle may have changed the visible binary
        // image; kick off its lazy fetch if not already loaded.
        self.ensure_review_image(&state).await;
        self.ui_state.modal = Modal::ReviewDiff(state);
    }

    /// Clear the decoded-image cache and bump the review generation. Called when
    /// a review opens so in-flight fetches from the previous review (which
    /// captured the old generation) are dropped on arrival rather than poisoning
    /// the new review's cache.
    pub(super) fn reset_review_images(&self) {
        self.review_images.borrow_mut().clear();
        self.review_image_gen
            .set(self.review_image_gen.get().wrapping_add(1));
    }

    /// Ensure the binary image for the currently-shown file+side is being (or
    /// has been) loaded. Inserts a `Pending` marker and spawns an off-thread
    /// fetch+decode that reports back via [`StateUpdate::ReviewImageLoaded`].
    /// Cheap no-op when the current file isn't an image or is already cached.
    pub(super) async fn ensure_review_image(&self, state: &DiffReviewState) {
        let Some(file) = state.current_file() else {
            return;
        };
        let Some(info) = file.binary.as_ref() else {
            return;
        };
        if !matches!(info.kind, crate::git::BinaryKind::Image { .. }) {
            return;
        }
        let side = shown_image_side(file, state.image_side);
        let path = side_path(file, side).to_string();
        let key = (path.clone(), side);

        if self.review_images.borrow().contains_key(&key) {
            return;
        }
        // Mark in-flight before the first await so repeated renders/keypresses
        // don't spawn duplicate fetches.
        self.review_images
            .borrow_mut()
            .insert(key, ImageEntry::Pending);

        let (worktree, base) = match self.service.review_blob_source(&state.session_id).await {
            Ok(src) => src,
            Err(e) => {
                self.review_images
                    .borrow_mut()
                    .insert((path, side), ImageEntry::Failed(e.to_string()));
                return;
            }
        };

        let tx = self.event_loop.sender();
        let generation = self.review_image_gen.get();
        tokio::spawn(async move {
            let bytes = match side {
                DiffSide::Old => crate::git::read_base_blob(&worktree, &base, &path).await,
                DiffSide::New => crate::git::read_worktree_file(&worktree, &path).await,
            };
            // Decode off the async runtime — it's CPU-bound.
            let image = match bytes {
                Err(e) => Err(format!("read failed: {e}")),
                Ok(b) => tokio::task::spawn_blocking(move || {
                    image::load_from_memory(&b)
                        .map(std::sync::Arc::new)
                        .map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(format!("decode task failed: {e}"))),
            };
            let _ = tx
                .send(AppEvent::StateUpdate(StateUpdate::ReviewImageLoaded {
                    generation,
                    path,
                    side,
                    image,
                }))
                .await;
        });
    }

    async fn handle_review_comment_key(&mut self, key: KeyEvent, state: &mut DiffReviewState) {
        use crossterm::event::{Event, KeyCode};
        let Some(draft) = state.comment.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                state.comment = None;
            }
            KeyCode::Enter => {
                let draft = state.comment.take().expect("comment present");
                if draft.input.value().trim().is_empty() {
                    return;
                }
                let range = draft.range;
                if let Some(ann) = state.build_draft(range, draft.input.value().to_string()) {
                    match self.service.create_comment(&state.session_id, ann).await {
                        Ok(_) => {
                            state.visual_anchor = None;
                            self.reload_review_comments(state).await;
                        }
                        Err(e) => self.set_review_status(&format!("Comment failed: {e}")),
                    }
                }
            }
            // Everything else (chars, backspace, delete, arrows, Home/End,
            // word/line shortcuts) is `tui-input`'s standard edit keymap.
            _ => {
                if let Some(req) = tui_input::backend::crossterm::to_input_request(&Event::Key(key))
                {
                    draft.input.handle(req);
                    state.follow_draft();
                }
            }
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

        // Label the session toggle with its actual (rebindable) key; omit the
        // hint entirely when no attach-capable binding exists.
        let toggle = crate::config::keybindings::review_toggle_binding(&self.config.keybindings)
            .map(|kb| format!("{kb} session · "))
            .unwrap_or_default();
        let hint = if state.comment.is_some() {
            " type comment · ←→/Home/End move · Enter save · Esc cancel ".to_string()
        } else if state.visual_anchor.is_some() {
            " ↑↓ extend · Enter/right-click comment · v/Esc cancel selection ".to_string()
        } else if state.focus == ReviewFocus::FileList {
            format!(
                " ↑↓/jk move · Enter expand/collapse · [ ] file · m reviewed · Tab to diff · {toggle}^Q/Esc close "
            )
        } else {
            // Offer the image side-toggle only when it does something: a binary
            // image with two sides (a modification).
            let image_toggle = if state.can_toggle_image_side() {
                "o before/after · "
            } else {
                ""
            };
            format!(
                " ↑↓/jk move · v select · Enter comment · z fold · d delete · m reviewed · a apply · r refresh · t layout · {image_toggle}{toggle}^Q/Esc close "
            )
        };
        // The footer doubles as this view's status bar — styled like the app
        // status bar so it reads as a replacement, not a second bar.
        frame.render_widget(
            Paragraph::new(Line::from(hint)).style(self.theme.status_bar()),
            rows[1],
        );
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
        // Inner content width (pane minus the two border columns), so a reviewed
        // row's background band can be padded to fill the full pane.
        let inner_width = area.width.saturating_sub(2) as usize;
        let mut lines: Vec<Line> = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let on_cursor = focused && i == state.tree_cursor;
            let line = match row {
                TreeRow::Dir {
                    depth,
                    name,
                    collapsed,
                    path,
                } => {
                    let indent = "  ".repeat(*depth);
                    let chevron = if *collapsed { '▶' } else { '▼' };
                    let mut spans = vec![Span::styled(
                        format!("{indent}{chevron} {name}"),
                        Style::default().fg(pal.dir_fg).add_modifier(Modifier::BOLD),
                    )];
                    spans.extend(comment_badge_span(state.dir_comment_count(path), &pal));
                    if on_cursor {
                        spans = select_spans(spans, &pal);
                    }
                    Line::from(spans)
                }
                TreeRow::File { depth, index, name } => {
                    let file = &state.diff.files[*index];
                    let reviewed = state.is_reviewed_path(file.display_path());
                    // Reviewed rows are dimmed so the remaining work stands out.
                    let dim = if reviewed {
                        Modifier::DIM
                    } else {
                        Modifier::empty()
                    };
                    let indent = "  ".repeat(*depth);
                    let marker = file_status_marker(file.status);
                    // Only the status letter is coloured; the file name stays
                    // the default foreground.
                    let mut spans = vec![
                        Span::raw(format!("{indent}  ")),
                        Span::styled(
                            marker.to_string(),
                            Style::default()
                                .fg(file_status_color(file.status, &pal))
                                .add_modifier(dim),
                        ),
                        Span::styled(format!(" {name}"), Style::default().add_modifier(dim)),
                    ];
                    spans.extend(reviewed_check_span(reviewed, &pal));
                    spans.extend(comment_badge_span(
                        state.comment_count(file.display_path()),
                        &pal,
                    ));
                    // Band reviewed rows so "read" files stand out; the cursor
                    // selection still wins on the focused row.
                    spans = apply_reviewed_bg(spans, reviewed, inner_width, &pal);
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
            .border_type(self.border_type())
            .border_style(Style::default().fg(border))
            .title(match state.reviewed.len() {
                0 => format!(" Files ({}) ", state.diff.files.len()),
                n => format!(" Files ({n}/{} reviewed) ", state.diff.files.len()),
            });
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
            Some(f) if state.is_reviewed_path(f.display_path()) => {
                format!(" {} — vs {} ✓ reviewed ", f.display_path(), state.base)
            }
            Some(f) => format!(" {} — vs {} ", f.display_path(), state.base),
            None => format!(" review — vs {} ", state.base),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(self.border_type())
            .border_style(Style::default().fg(border))
            .title(title);

        // Binary files (images, blobs) carry no textual hunks — render the image
        // (or a placeholder) instead of the line-based diff body.
        if let Some(file) = state.current_file()
            && let Some(info) = file.binary.as_ref()
        {
            self.render_review_binary(frame, area, block, state, file, info);
            return;
        }

        // Syntax highlighting emits RGB foregrounds, so only apply it on
        // true-color terminals; otherwise fall back to the palette text colour.
        let highlight = self.theme.mode == ColorMode::TrueColor;
        let ext = state
            .current_file()
            .map(|f| file_extension(f.display_path()).to_string())
            .unwrap_or_default();
        let width = area.width.saturating_sub(2) as usize;
        // Record the content width so keypress-time cursor/scroll math wraps
        // lines exactly as we render them here.
        state.body_width.set(width);
        let segs = state.word_segments();
        let rounded = self.config.rounded_borders;
        let lines = match state.layout {
            ReviewLayout::Inline => {
                review_body_lines(state, focused, &pal, &ext, highlight, width, &segs, rounded)
            }
            ReviewLayout::SideBySide => review_body_lines_side_by_side(
                state, focused, &pal, &ext, highlight, width, &segs, rounded,
            ),
        };

        frame.render_widget(
            Paragraph::new(lines).block(block).scroll((state.scroll, 0)),
            area,
        );
    }

    /// Render a binary file's review body: the decoded image via `ratatui-image`
    /// (graphics protocol or half-block fallback), or a placeholder for
    /// non-image blobs and not-yet-loaded images. Bytes are fetched lazily — see
    /// `ensure_review_image`.
    fn render_review_binary(
        &self,
        frame: &mut Frame,
        area: Rect,
        block: Block<'static>,
        state: &DiffReviewState,
        file: &FileDiff,
        info: &crate::git::BinaryInfo,
    ) {
        let pal = self.theme.review_palette();
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let side = shown_image_side(file, state.image_side);
        let size = match side {
            DiffSide::Old => info.old_size,
            DiffSide::New => info.new_size,
        };
        let side_label = match side {
            DiffSide::Old => "before",
            DiffSide::New => "after",
        };

        if !matches!(info.kind, crate::git::BinaryKind::Image { .. }) {
            let note = format!(
                "Binary file · {} · not a previewable image",
                human_size(size)
            );
            frame.render_widget(
                Paragraph::new(note)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(pal.text)),
                inner,
            );
            return;
        }

        // Caption (one row) above the image area.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(inner);
        let caption = image_caption(file.status, side_label, size);
        frame.render_widget(
            Paragraph::new(caption).style(Style::default().fg(pal.gutter_fg)),
            rows[0],
        );

        let path = side_path(file, side).to_string();
        let mut images = self.review_images.borrow_mut();
        match images.get_mut(&(path, side)) {
            Some(ImageEntry::Ready(proto)) => {
                frame.render_stateful_widget(
                    ratatui_image::StatefulImage::default(),
                    rows[1],
                    proto.as_mut(),
                );
            }
            Some(ImageEntry::Failed(e)) => {
                frame.render_widget(
                    Paragraph::new(format!("Failed to load image: {e}"))
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(pal.del_fg)),
                    rows[1],
                );
            }
            // Pending, or not yet requested (the request is kicked off by the
            // navigation handlers, which insert `Pending` before this renders).
            _ => {
                frame.render_widget(
                    Paragraph::new("Loading image…")
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(pal.gutter_fg)),
                    rows[1],
                );
            }
        }
    }
}

/// Caption shown above a review image: side label + size, plus a `press o to
/// toggle` hint only when the file is a modification (the sole case where `o`
/// has two sides to flip between — added/deleted files are single-sided).
fn image_caption(status: FileStatus, side_label: &str, size: Option<u64>) -> String {
    if status == FileStatus::Modified {
        format!(" {side_label} · {} · press o to toggle ", human_size(size))
    } else {
        format!(" {side_label} · {} ", human_size(size))
    }
}

/// Human-readable byte size for an optional count (`None` → `"? bytes"`).
fn human_size(size: Option<u64>) -> String {
    match size {
        None => "? bytes".to_string(),
        Some(n) if n < 1024 => format!("{n} bytes"),
        Some(n) if n < 1024 * 1024 => format!("{:.1} KiB", n as f64 / 1024.0),
        Some(n) => format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0)),
    }
}

/// Normalise a review-view key into the keycode the dispatch matches on.
/// `Ctrl-n`/`Ctrl-p` are folded onto `Down`/`Up` so they act as navigation
/// aliases for the arrow keys (and `j`/`k`), mirroring the other list modals;
/// every other key passes through unchanged.
/// Telemetry feature name for a review-view key, or `None` for keys with no
/// tracked feature (navigation, scroll, file movement — pure noise).
///
/// These are UI-only actions handled directly in [`Self::handle_review_key`];
/// unlike the comment create/delete/apply mutations they never reach an
/// instrumented [`CommanderService`] method, so they are recorded inline. The
/// `code` is the navigation-normalised keycode (post [`review_nav_keycode`]),
/// and `focus` gates `v`, which only enters visual mode in the body. Kept pure
/// and free-standing so it is unit-testable without driving the async handler.
///
/// `o` (image-side toggle) is recorded separately in the handler because
/// whether it does anything depends on the current file (see
/// [`DiffReviewState::can_toggle_image_side`]), which this key-only mapping
/// can't see — counting every `o` would inflate the metric with no-op presses.
fn review_key_feature(code: crossterm::event::KeyCode, focus: ReviewFocus) -> Option<&'static str> {
    use crossterm::event::KeyCode;
    match code {
        KeyCode::Char('t') => Some("review.toggle_layout"),
        KeyCode::Char('z') => Some("review.toggle_fold"),
        KeyCode::Char('r') => Some("review.refresh"),
        KeyCode::Char('v') if focus == ReviewFocus::Body => Some("review.visual_select"),
        _ => None,
    }
}

fn review_nav_keycode(key: crossterm::event::KeyEvent) -> crossterm::event::KeyCode {
    use crossterm::event::{KeyCode, KeyModifiers};
    match key.code {
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyCode::Down,
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyCode::Up,
        other => other,
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

/// The ` *N` pending-comment badge for a file-tree row, comment-coloured so it
/// stands out from the file name, or `None` when there are no pending comments.
fn comment_badge_span(count: usize, pal: &ReviewPalette) -> Option<Span<'static>> {
    (count > 0).then(|| {
        Span::styled(
            format!(" {COMMENT_MARKER}{count}"),
            Style::default().fg(pal.comment_border),
        )
    })
}

/// The ` ✓` reviewed check for a file-tree row, add-coloured so it reads as
/// "done", or `None` when the file is not marked reviewed.
fn reviewed_check_span(reviewed: bool, pal: &ReviewPalette) -> Option<Span<'static>> {
    reviewed.then(|| Span::styled(" ✓", Style::default().fg(pal.add_fg)))
}

/// Lay the subtle "read" background band across a reviewed file-tree row,
/// padding the line out to `width` so the band fills the whole pane rather than
/// only sitting behind the text. Returns the spans untouched when the row isn't
/// reviewed, so unread files (where the work is) keep the default background.
fn apply_reviewed_bg(
    mut spans: Vec<Span<'static>>,
    reviewed: bool,
    width: usize,
    pal: &ReviewPalette,
) -> Vec<Span<'static>> {
    if !reviewed {
        return spans;
    }
    for span in &mut spans {
        span.style = span.style.bg(pal.reviewed_bg);
    }
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if width > used {
        spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(pal.reviewed_bg),
        ));
    }
    spans
}

/// Build the inline-rendered body for the current file: hunk headers plus each
/// diff line with a coloured gutter, full-width add/remove background fill,
/// word-level intra-line highlight, and an comment marker. Selected lines
/// (cursor or visual range) are reversed when the body is focused.
#[allow(clippy::too_many_arguments)]
fn review_body_lines(
    state: &DiffReviewState,
    focused: bool,
    pal: &ReviewPalette,
    ext: &str,
    highlight: bool,
    width: usize,
    segs: &[WordSegs],
    rounded: bool,
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

            // Code content as its own span list, so it can soft-wrap independent
            // of the fixed-width gutter.
            let mut content_spans = Vec::new();
            for (text, emph) in &segs[idx] {
                let bg = if *emph { emph_bg } else { line_bg };
                push_segment(&mut content_spans, text, ext, highlight, pal.text, bg);
            }

            // First row carries the line numbers + sign; wrap continuations get a
            // blank gutter of the same width so the coloured fill stays aligned.
            let gutter = |first: bool| -> Vec<Span<'static>> {
                vec![
                    Span::styled(" ", Style::default().bg(emph_bg)),
                    if first {
                        Span::styled(
                            format!("{ann}{old} {new} "),
                            Style::default().fg(pal.gutter_fg).bg(gutter_bg),
                        )
                    } else {
                        Span::styled(" ".repeat(11), Style::default().bg(gutter_bg))
                    },
                    // Sign + a space so the code doesn't butt against the +/-.
                    if first {
                        Span::styled(format!("{sign} "), Style::default().fg(sign_fg).bg(line_bg))
                    } else {
                        Span::styled("  ", Style::default().bg(line_bg))
                    },
                ]
            };

            let content_width = inline_content_width(width);
            let wrapped = wrap_spans(content_spans, content_width);
            let selected = focused && idx >= sel_lo && idx <= sel_hi;
            for (c, content_row) in wrapped.into_iter().enumerate() {
                let mut spans = gutter(c == 0);
                spans.extend(content_row);
                let mut spans = fit_spans(spans, width, line_bg);
                if selected {
                    spans = select_spans(spans, pal);
                }
                out.push(Line::from(spans));
            }

            // Inline comment box(es) anchored to this line.
            if let Some(anns) = anchors.get(&idx) {
                for ann in anns {
                    out.extend(comment_box_lines(
                        ann,
                        state.is_comment_collapsed(ann.id),
                        width,
                        pal,
                        rounded,
                    ));
                }
            }
            // The in-progress edit box renders where the saved comment will.
            if state.draft_anchor() == Some(idx)
                && let Some(draft) = state.comment.as_ref()
            {
                out.extend(comment_draft_box_lines(
                    &super::input_with_caret(&draft.input),
                    &draft_loc_label(state, draft.range),
                    width,
                    pal,
                    rounded,
                ));
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

/// Word-wrap styled content `spans` into rows of at most `width` display
/// columns. The break points come from [`wrap_row_lens`] (whitespace-aware,
/// hard-splitting only over-long words), and the span stream is re-sliced by
/// those row lengths — so styling is preserved across a break, even when it
/// falls inside a span. Always returns at least one row.
fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    let text: String = spans.iter().flat_map(|s| s.content.chars()).collect();
    let lens = wrap_row_lens(&text, width);
    if lens.len() <= 1 {
        return vec![spans];
    }
    let mut rows: Vec<Vec<Span<'static>>> = Vec::with_capacity(lens.len());
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_idx = 0;
    let mut row_remaining = lens[0];
    for span in spans {
        let style = span.style;
        let mut buf = String::new();
        for ch in span.content.chars() {
            while row_remaining == 0 {
                if !buf.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut buf), style));
                }
                rows.push(std::mem::take(&mut row));
                row_idx += 1;
                row_remaining = lens.get(row_idx).copied().unwrap_or(usize::MAX);
            }
            buf.push(ch);
            row_remaining -= 1;
        }
        if !buf.is_empty() {
            row.push(Span::styled(buf, style));
        }
    }
    rows.push(row);
    rows
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
    rounded: bool,
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

    // Corners follow the user's rounded-borders setting, matching the panes.
    let (tl, tr, bl, br) = if rounded {
        ('╭', '╮', '╰', '╯')
    } else {
        ('┌', '┐', '└', '┘')
    };

    let mut out = Vec::new();
    let header = hrule(&format!("{chevron} {marker}comment "), inner);
    out.push(Line::from(Span::styled(
        format!("{INDENT}{tl}{header}{tr}"),
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
        format!("{INDENT}{bl}{}{br}", "─".repeat(inner)),
        border,
    )));
    out
}

/// A short label for the line(s) the in-progress comment covers, using the
/// displayed gutter numbers (e.g. `line 5` / `lines 5–8`), for the edit box
/// title. Mirrors the label the old bottom overlay showed.
fn draft_loc_label(state: &DiffReviewState, range: (usize, usize)) -> String {
    match state.resolved_line_range(range) {
        Some((lo, hi)) if lo == hi => format!("line {lo}"),
        Some((lo, hi)) => format!("lines {lo}–{hi}"),
        None => "line ?".to_string(),
    }
}

/// Number of rendered rows [`comment_draft_box_lines`] produces for the same
/// `display`/`width`. Drives the inline layout model so cursor/scroll/click stay
/// in step with the rendered edit box (guarded by a test). `display` is the
/// caret-embedded text from [`super::input_with_caret`].
fn comment_draft_box_height(display: &str, width: usize) -> usize {
    const INDENT_LEN: usize = 2; // "  "
    let avail = width.saturating_sub(INDENT_LEN);
    if avail < 8 {
        return 0;
    }
    let inner = avail - 2;
    let text_width = inner.saturating_sub(2);
    let body = wrap_text(display, text_width).len();
    // Top border + wrapped body lines + bottom border.
    body + 2
}

/// Render the in-progress comment as an inline edit box, anchored where the
/// saved comment will appear. Same geometry as [`comment_box_lines`]'s expanded
/// form (so the layout model can share width math) but with the draft border
/// colour, a `*`-marked title carrying the line range, and a caret at the
/// cursor. `display` is the caret-embedded text from [`super::input_with_caret`].
fn comment_draft_box_lines(
    display: &str,
    loc: &str,
    width: usize,
    pal: &ReviewPalette,
    rounded: bool,
) -> Vec<Line<'static>> {
    const INDENT: &str = "  ";
    let avail = width.saturating_sub(INDENT.len());
    if avail < 8 {
        return Vec::new();
    }
    let inner = avail - 2;
    let border = Style::default().fg(pal.draft_border);
    let (tl, tr, bl, br) = if rounded {
        ('╭', '╮', '╰', '╯')
    } else {
        ('┌', '┐', '└', '┘')
    };

    let mut out = Vec::new();
    let header = hrule(&format!("{COMMENT_MARKER} comment · {loc} "), inner);
    out.push(Line::from(Span::styled(
        format!("{INDENT}{tl}{header}{tr}"),
        border,
    )));
    let text_width = inner.saturating_sub(2);
    for chunk in wrap_text(display, text_width) {
        let body: String = chunk.chars().take(text_width).collect();
        out.push(Line::from(vec![
            Span::styled(format!("{INDENT}│"), border),
            Span::raw(format!(" {body:<text_width$} ")),
            Span::styled("│".to_string(), border),
        ]));
    }
    out.push(Line::from(Span::styled(
        format!("{INDENT}{bl}{}{br}", "─".repeat(inner)),
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
pub(crate) type WordSegs = Vec<(String, bool)>;

/// Shared empty segment list, returned by [`DiffReviewState::word_segments`]
/// when the current file index is out of range (only possible for an empty
/// diff) so the `Ref::map` closure always has something to borrow.
fn empty_segs() -> &'static Vec<WordSegs> {
    static EMPTY: std::sync::OnceLock<Vec<WordSegs>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Vec::new)
}

/// Precompute every file's word-diff segments and (on true-color terminals) warm
/// the shared syntax-highlight cache for every content line. This is the heavy,
/// per-file work the review body would otherwise do lazily on first navigation —
/// the LCS pass per file plus a fresh syntect highlighter per line. It's pure
/// (reads the parsed diff, writes only the process-global highlight cache) so the
/// open-review flow runs it on a blocking worker thread while a spinner shows,
/// making the first view of each file instant. Returns the per-file segments in
/// `diff.files` order, ready for [`DiffReviewState::prime_segments`].
pub(crate) fn precompute_review_caches(
    diff: &ParsedDiff,
    highlight: bool,
    text_fg: Color,
) -> Vec<Vec<WordSegs>> {
    if highlight {
        // Warm every content line across all files in one parallel pass. The
        // flattened line list spreads a single large file across cores too, not
        // just many files. `ext` borrows from each file's `display_path` (both
        // `&str`), which lives as long as `diff`.
        let lines: Vec<(&str, &str)> = diff
            .files
            .iter()
            .flat_map(|file| {
                let ext = file_extension(file.display_path());
                file.hunks
                    .iter()
                    .flat_map(move |hunk| hunk.lines.iter().map(move |l| (ext, l.content.as_str())))
            })
            .collect();
        warm_highlight_cache(&lines, text_fg);
    }
    // `par_iter().collect()` preserves order, so the result still lines up with
    // `diff.files` for `prime_segments`.
    diff.files.par_iter().map(word_diff_segments).collect()
}

/// Review payload prepared off the render thread: the parsed diff plus its warmed
/// render caches, ready to construct a [`DiffReviewState`]. Built by the
/// open-review background task (see `handle_open_review`) and consumed by
/// `handle_state_update` once the loading spinner can be replaced with the view.
#[derive(Debug, Clone)]
pub struct ReviewPrepared {
    pub(super) session_id: SessionId,
    pub(super) title: String,
    pub(super) base: String,
    pub(super) diff: ParsedDiff,
    pub(super) comments: Vec<Comment>,
    pub(super) reviewed: Vec<String>,
    pub(super) segments: Vec<Vec<WordSegs>>,
    pub(super) content_hash: u64,
}

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
#[allow(clippy::too_many_arguments)]
fn review_body_lines_side_by_side(
    state: &DiffReviewState,
    focused: bool,
    pal: &ReviewPalette,
    ext: &str,
    highlight: bool,
    width: usize,
    segs: &[WordSegs],
    rounded: bool,
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
                                rounded,
                            ));
                        }
                    }
                    // The in-progress edit box renders where the comment will.
                    if state.draft_anchor() == Some(sel)
                        && let Some(draft) = state.comment.as_ref()
                    {
                        out.extend(comment_draft_box_lines(
                            &super::input_with_caret(&draft.input),
                            &draft_loc_label(state, draft.range),
                            width,
                            pal,
                            rounded,
                        ));
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
    fn review_key_feature_maps_tracked_actions() {
        use crossterm::event::KeyCode;
        // The UI-only toggles each record a feature regardless of focus.
        // `o` is intentionally absent: it's gated on file type in the handler
        // (see can_toggle_image_side) so no-op presses aren't counted.
        for (code, feature) in [
            (KeyCode::Char('t'), "review.toggle_layout"),
            (KeyCode::Char('z'), "review.toggle_fold"),
            (KeyCode::Char('r'), "review.refresh"),
        ] {
            assert_eq!(
                review_key_feature(code, ReviewFocus::FileList),
                Some(feature)
            );
            assert_eq!(review_key_feature(code, ReviewFocus::Body), Some(feature));
        }
        // `v` only enters visual selection (and records) in the body.
        assert_eq!(
            review_key_feature(KeyCode::Char('v'), ReviewFocus::Body),
            Some("review.visual_select")
        );
        assert_eq!(
            review_key_feature(KeyCode::Char('v'), ReviewFocus::FileList),
            None
        );
        // Navigation / scroll / file-movement keys are noise — never recorded.
        // `o` is here too: it's recorded inline (gated), not via this mapping.
        for code in [
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('['),
            KeyCode::Char(']'),
            KeyCode::Tab,
            KeyCode::Enter,
            KeyCode::Char('m'),
            KeyCode::Char('a'),
            KeyCode::Char('d'),
            KeyCode::Char('o'),
        ] {
            assert_eq!(
                review_key_feature(code, ReviewFocus::Body),
                None,
                "{code:?}"
            );
        }
    }

    #[test]
    fn can_toggle_image_side_gates_on_modified_image() {
        use crate::git::{BinaryInfo, BinaryKind};

        let mut state = state_with_two_files();
        // a.rs is a modified *text* file — the image toggle does nothing.
        assert!(!state.can_toggle_image_side());

        // Make the current file a binary image.
        let file = &mut state.diff.files[state.selected_file];
        file.binary = Some(BinaryInfo {
            kind: BinaryKind::Image {
                mime: "image/png".to_string(),
            },
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
        });

        // A modified binary image has two sides: the toggle is meaningful.
        file.status = FileStatus::Modified;
        assert!(state.can_toggle_image_side());

        // An added image shows only its one side — no-op, not counted.
        state.diff.files[state.selected_file].status = FileStatus::Added;
        assert!(!state.can_toggle_image_side());
    }

    #[test]
    fn ctrl_n_p_alias_arrow_keys_for_navigation() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        // Ctrl-n / Ctrl-p fold onto Down / Up so they navigate like the
        // arrow keys (and j/k) regardless of focus.
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(review_nav_keycode(ctrl('n')), KeyCode::Down);
        assert_eq!(review_nav_keycode(ctrl('p')), KeyCode::Up);
        // Without Ctrl, and for unrelated keys, the keycode passes through.
        let plain = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(review_nav_keycode(plain('n')), KeyCode::Char('n'));
        assert_eq!(review_nav_keycode(plain('j')), KeyCode::Char('j'));
        assert_eq!(
            review_nav_keycode(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            KeyCode::Esc
        );
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
    fn refresh_diff_swaps_content_and_keeps_file_by_path() {
        let mut s = state_with_two_files();
        // Viewing b.rs (the second file).
        s.selected_file = 1;
        s.focus = ReviewFocus::Body;
        assert_eq!(s.current_file().unwrap().display_path(), "b.rs");

        // Simulate the agent editing files after comments were applied: a fresh
        // diff where b.rs gained a line and the file order changed.
        let new_diff = parse_unified_diff(
            "\
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1,2 @@
-b
+B
+extra
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 fn main() {
+    let y = 3;
 }
",
        );
        s.refresh_diff(new_diff, Vec::new(), HashSet::new(), Vec::new(), 42);

        assert_eq!(s.content_hash, 42);
        // The body still shows b.rs even though it moved to index 0...
        assert_eq!(s.selected_file, 0);
        assert_eq!(s.current_file().unwrap().display_path(), "b.rs");
        // ...and it reflects the *new* content (the stale snapshot lacked it).
        let body: Vec<_> = s
            .selectable_lines()
            .iter()
            .map(|l| l.content.clone())
            .collect();
        assert!(
            body.iter().any(|c| c == "extra"),
            "fresh diff shown: {body:?}"
        );
    }

    #[test]
    fn refresh_diff_clamps_cursor_into_shorter_file() {
        let mut s = state_with_two_files();
        s.selected_file = 0; // a.rs has 3 selectable lines
        s.focus = ReviewFocus::Body;
        s.cursor = 2;
        // a.rs shrinks to a single changed line in the refreshed diff.
        let new_diff = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-fn main() {}
+fn main() { }
",
        );
        s.refresh_diff(new_diff, Vec::new(), HashSet::new(), Vec::new(), 7);
        assert_eq!(s.current_file().unwrap().display_path(), "a.rs");
        assert_eq!(s.selectable_count(), 2);
        assert!(
            s.cursor < s.selectable_count(),
            "cursor clamped: {}",
            s.cursor
        );
    }

    #[test]
    fn refresh_diff_falls_back_when_file_removed() {
        let mut s = state_with_two_files();
        s.selected_file = 1; // b.rs
        // Refreshed diff no longer contains b.rs.
        let new_diff = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 fn main() {
+    let y = 3;
 }
",
        );
        s.refresh_diff(new_diff, Vec::new(), HashSet::new(), Vec::new(), 1);
        assert_eq!(s.diff.files.len(), 1);
        assert_eq!(s.selected_file, 0);
        assert_eq!(s.current_file().unwrap().display_path(), "a.rs");
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

    // --- reviewed marks ---

    fn state_with_three_files() -> DiffReviewState {
        let diff = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-a
+A
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-b
+B
diff --git a/c.rs b/c.rs
--- a/c.rs
+++ b/c.rs
@@ -1 +1 @@
-c
+C
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
    fn mark_advances_to_next_unreviewed_file() {
        let mut s = state_with_two_files();
        s.set_reviewed("a.rs".to_string(), true);
        s.advance_to_next_unreviewed();
        assert!(s.is_reviewed_path("a.rs"));
        assert_eq!(s.selected_file, 1);
    }

    #[test]
    fn advance_wraps_past_end_to_first_unreviewed() {
        let mut s = state_with_three_files();
        s.set_reviewed("b.rs".to_string(), true);
        s.set_reviewed("c.rs".to_string(), true);
        s.selected_file = 1;
        s.advance_to_next_unreviewed();
        assert_eq!(s.selected_file, 0, "wraps past reviewed c.rs to a.rs");
    }

    #[test]
    fn advance_stays_put_when_all_reviewed() {
        let mut s = state_with_two_files();
        s.set_reviewed("a.rs".to_string(), true);
        s.set_reviewed("b.rs".to_string(), true);
        s.advance_to_next_unreviewed();
        assert_eq!(s.selected_file, 0);
    }

    #[test]
    fn unmark_does_not_advance() {
        let mut s = state_with_two_files();
        s.set_reviewed("a.rs".to_string(), true);
        s.set_reviewed("a.rs".to_string(), false);
        assert!(!s.is_reviewed_path("a.rs"));
        assert_eq!(s.selected_file, 0, "unmarking never moves the selection");
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
            binary: None,
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
    fn double_click_selects_line_then_opens_comment() {
        let mut s = state_with_two_files();
        let body = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        // Double-click body row 2 (selectable index 1): selects just that line
        // and opens its comment box, like a right-click on a fresh line.
        assert!(s.double_click_comment(5, 2, body));
        assert_eq!(s.cursor, 1);
        assert!(s.visual_anchor.is_none());
        let draft = s.comment.as_ref().unwrap();
        assert_eq!(draft.range, (1, 1));
    }

    #[test]
    fn double_click_on_header_row_is_no_op() {
        let mut s = state_with_two_files();
        let body = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        // Body row 0 is the hunk header — not a selectable diff line.
        assert!(!s.double_click_comment(5, 0, body));
        assert!(s.comment.is_none());
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
    fn comment_badge_span_is_comment_coloured_and_hidden_when_zero() {
        let pal = Theme::truecolor().review_palette();
        assert!(comment_badge_span(0, &pal).is_none());
        let span = comment_badge_span(3, &pal).expect("badge for non-zero count");
        assert_eq!(span.content.as_ref(), format!(" {COMMENT_MARKER}3"));
        assert_eq!(span.style.fg, Some(pal.comment_border));
    }

    #[test]
    fn reviewed_check_span_hidden_when_unreviewed() {
        let pal = Theme::truecolor().review_palette();
        assert!(reviewed_check_span(false, &pal).is_none());
        let span = reviewed_check_span(true, &pal).expect("check for reviewed file");
        assert_eq!(span.content.as_ref(), " ✓");
        assert_eq!(span.style.fg, Some(pal.add_fg));
    }

    #[test]
    fn reviewed_bg_unreviewed_row_unchanged() {
        let pal = Theme::truecolor().review_palette();
        let spans = vec![Span::raw("a.rs")];
        let out = apply_reviewed_bg(spans, false, 20, &pal);
        // Unreviewed rows keep the default background and no padding span.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].style.bg, None);
    }

    #[test]
    fn reviewed_bg_bands_and_pads_to_width() {
        let pal = Theme::truecolor().review_palette();
        let spans = vec![Span::raw("a.rs")]; // 4 columns
        let out = apply_reviewed_bg(spans, true, 20, &pal);
        // Every span carries the reviewed background...
        assert!(out.iter().all(|s| s.style.bg == Some(pal.reviewed_bg)));
        // ...and the band is padded out to fill the full pane width.
        let total: usize = out.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(total, 20);
    }

    #[test]
    fn reviewed_bg_no_pad_when_content_exceeds_width() {
        let pal = Theme::truecolor().review_palette();
        let spans = vec![Span::raw("a-really-long-file-name.rs")]; // 26 columns
        let out = apply_reviewed_bg(spans, true, 10, &pal);
        // No padding span is added when the content already overflows the width.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].style.bg, Some(pal.reviewed_bg));
    }

    #[test]
    fn dir_comment_count_aggregates_subtree_pending_comments() {
        let mut s = state_with_two_files();
        s.comments.push(Comment::new(
            "src/git/diff.rs",
            CommentSide::New,
            (1, 1),
            "x",
            "note",
        ));
        s.comments.push(Comment::new(
            "src/git/backend.rs",
            CommentSide::New,
            (1, 1),
            "y",
            "note",
        ));
        // Both nested comments roll up to their ancestor directories.
        assert_eq!(s.dir_comment_count("src"), 2);
        assert_eq!(s.dir_comment_count("src/git"), 2);
        // A sibling directory with no comments stays clean, and the prefix
        // match respects the `/` boundary (no "src" → "srcfoo" leakage).
        assert_eq!(s.dir_comment_count("other"), 0);
        assert_eq!(s.dir_comment_count("src/gi"), 0);
        // Applied comments don't count.
        s.comments[0].status = CommentStatus::Applied;
        assert_eq!(s.dir_comment_count("src/git"), 1);
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
    fn orphaned_drifted_comment_pins_to_last_line_and_stays_reachable() {
        // Regression: a drifted comment whose anchor line no longer exists in
        // the diff (the code drifted away) used to render nowhere — invisible
        // and impossible to delete, yet still counted as pending and blocking
        // Apply. It must pin to the file's last selectable line so it stays
        // visible, gutter-marked, and selectable for deletion.
        let mut s = state_with_two_files();
        // a.rs has 3 selectable lines (indices 0..=2); new line 99 is gone.
        let mut ann = Comment::new("a.rs", CommentSide::New, (99, 99), "vanished", "note");
        ann.status = CommentStatus::Drifted;
        s.comments.push(ann);
        s.focus = ReviewFocus::Body;

        // The box anchors to the last selectable line rather than being dropped.
        let anchors = s.comment_anchors();
        assert_eq!(anchors.get(&2).map(|v| v.len()), Some(1));

        // The last line carries a drift-flagged gutter marker...
        assert_eq!(s.comment_marker(2), Some(true));

        // ...and the cursor there resolves to the comment so `d` can delete it.
        s.cursor = 2;
        assert!(s.comment_at_cursor().is_some());
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
        assert_eq!(comment_box_lines(&ann, true, 60, &pal, true).len(), 1);
        // top border + two comment paragraphs + bottom border.
        assert_eq!(comment_box_lines(&ann, false, 60, &pal, true).len(), 4);
    }

    #[test]
    fn comment_box_corners_follow_rounded_setting() {
        let pal = Theme::truecolor().review_palette();
        let ann = Comment::new("a.rs", CommentSide::New, (2, 2), "let y = 3;", "note");
        let corners = |rounded: bool| -> String {
            let lines = comment_box_lines(&ann, false, 60, &pal, rounded);
            let top: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
            let bot: String = lines
                .last()
                .unwrap()
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            format!("{top}{bot}")
        };
        let round = corners(true);
        assert!(round.contains('╭') && round.contains('╮'));
        assert!(round.contains('╰') && round.contains('╯'));
        let square = corners(false);
        assert!(square.contains('┌') && square.contains('┐'));
        assert!(square.contains('└') && square.contains('┘'));
        assert!(!square.contains('╭') && !square.contains('╯'));
    }

    #[test]
    fn comment_box_header_drops_asterisk_keeps_drift_marker() {
        let pal = Theme::truecolor().review_palette();
        let text_of = |lines: &[Line]| -> String {
            lines[0].spans.iter().map(|s| s.content.as_ref()).collect()
        };

        // A staged comment's box header has no asterisk (the gutter keeps it).
        let mut ann = Comment::new("a.rs", CommentSide::New, (2, 2), "let y = 3;", "note");
        let expanded = text_of(&comment_box_lines(&ann, false, 60, &pal, true));
        let collapsed = text_of(&comment_box_lines(&ann, true, 60, &pal, true));
        assert!(expanded.contains("comment"));
        assert!(!expanded.contains(COMMENT_MARKER));
        assert!(!collapsed.contains(COMMENT_MARKER));

        // A drifted comment still surfaces the ⚠ in its box header.
        ann.status = CommentStatus::Drifted;
        let drifted = text_of(&comment_box_lines(&ann, false, 60, &pal, true));
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
                        comment_box_lines(&ann, collapsed, width, &pal, true).len(),
                        "height mismatch (collapsed={collapsed}, width={width})"
                    );
                }
            }
        }
    }

    #[test]
    fn comment_draft_box_height_matches_rendered() {
        let pal = Theme::truecolor().review_palette();
        // Cover end-of-text and mid-text caret placement (the caret is spliced
        // into `display` before either function wraps it).
        for display in [
            "▏",
            "short▏",
            "mid▏dle",
            &format!("{}▏", "word ".repeat(40)),
        ] {
            for width in [12usize, 40, 80, 6 /* below the 8-col floor */] {
                assert_eq!(
                    comment_draft_box_height(display, width),
                    comment_draft_box_lines(display, "line 1", width, &pal, true).len(),
                    "height mismatch (width={width}, display={display:?})"
                );
            }
        }
    }

    #[test]
    fn draft_box_renders_inline_at_its_anchor() {
        // Opening a comment box must add rows to the inline body, anchored after
        // the selected line — and the renderer and layout model must still agree.
        let mut s = state_with_two_files();
        s.body_width.set(80);
        s.focus = ReviewFocus::Body;
        s.cursor = 1; // the inserted line
        assert!(s.begin_comment());

        let rows = s.inline_physical_rows();
        let draft_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, BodyRow::Draft))
            .map(|(i, _)| i)
            .collect();
        assert!(!draft_rows.is_empty(), "draft box should occupy body rows");
        // The draft rows sit immediately after selectable line 1's row.
        let line_row = s.body_row_of(1);
        assert_eq!(draft_rows[0], line_row + 1);

        // Renderer row count matches the layout model (cursor/scroll/click).
        let pal = Theme::truecolor().review_palette();
        let segs = s.word_segments();
        let lines = review_body_lines(&s, true, &pal, "rs", true, 80, &segs, true);
        assert_eq!(lines.len(), rows.len());
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
    fn precompute_matches_per_file_word_diff_segments() {
        let s = state_with_two_files();
        // Highlighting on exercises the cache-warming branch too (the returned
        // segments must be identical regardless).
        let pre = precompute_review_caches(&s.diff, true, Color::Reset);
        assert_eq!(pre.len(), s.diff.files.len());
        for (i, file) in s.diff.files.iter().enumerate() {
            assert_eq!(pre[i], word_diff_segments(file), "file {i} segments differ");
        }
    }

    #[test]
    fn primed_segments_are_returned_without_recompute() {
        let mut s = state_with_two_files();
        // Prime with sentinel data distinct from any real computation: if
        // word_segments recomputed, it would not match these.
        let sentinel: Vec<Vec<WordSegs>> = (0..s.diff.files.len())
            .map(|i| vec![vec![(format!("PRIMED-{i}"), false)]])
            .collect();
        s.prime_segments(sentinel.clone());
        s.selected_file = 0;
        assert_eq!(*s.word_segments(), sentinel[0]);
        s.selected_file = 1;
        assert_eq!(*s.word_segments(), sentinel[1]);
    }

    #[test]
    fn priming_with_real_precompute_matches_lazy_path() {
        let mut s = state_with_two_files();
        let pre = precompute_review_caches(&s.diff, false, Color::Reset);
        s.prime_segments(pre);
        for i in 0..s.diff.files.len() {
            s.selected_file = i;
            assert_eq!(*s.word_segments(), word_diff_segments(&s.diff.files[i]));
        }
    }

    #[test]
    fn word_segments_on_empty_diff_is_empty_without_panic() {
        let s = DiffReviewState::new(
            SessionId::new(),
            "test".to_string(),
            "HEAD".to_string(),
            parse_unified_diff(""),
            Vec::new(),
        );
        assert!(s.word_segments().is_empty());
    }

    fn state_with_long_line() -> DiffReviewState {
        // One short context line and one long addition (16 content columns).
        let diff = parse_unified_diff(
            "\
diff --git a/x.rs b/x.rs
--- a/x.rs
+++ b/x.rs
@@ -1 +1,2 @@
 ctx
+0123456789ABCDEF
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
    fn wrap_row_lens_breaks_at_whole_words() {
        // "hello world foo" at width 8 → "hello " / "world " / "foo".
        assert_eq!(wrap_row_lens("hello world foo", 8), vec![6, 6, 3]);
    }

    #[test]
    fn wrap_row_lens_hard_splits_overlong_words() {
        // No whitespace to break on, so fall back to filling each row.
        assert_eq!(wrap_row_lens("abcdefghij", 4), vec![4, 4, 2]);
    }

    #[test]
    fn wrap_spans_word_wraps_and_preserves_text() {
        // Break falls inside the second span; styling and all characters are
        // preserved, and it wraps at the space (whole words).
        let rows = wrap_spans(
            vec![
                Span::raw("hello ".to_string()),
                Span::raw("world foo".to_string()),
            ],
            8,
        );
        let texts: Vec<String> = rows
            .iter()
            .map(|r| r.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(texts, vec!["hello ", "world ", "foo"]);
    }

    #[test]
    fn line_wrap_rows_matches_wrap_spans_count() {
        for text in ["", "x", "abc", "hello world foo", "abcdefghij", "a b c d e"] {
            let spans = vec![Span::raw(text.to_string())];
            assert_eq!(
                wrap_spans(spans, 4).len(),
                line_wrap_rows(text, 4),
                "text {text:?}"
            );
        }
        // Degenerate zero width is one (clipped) row, not an infinite wrap.
        assert_eq!(line_wrap_rows("0123456789", 0), 1);
        assert_eq!(
            wrap_spans(vec![Span::raw("0123456789".to_string())], 0).len(),
            1
        );
    }

    #[test]
    fn long_line_wraps_and_mappings_stay_consistent() {
        let s = state_with_long_line();
        // Body width chosen so the content (wrap) width is exactly 8 columns.
        s.body_width
            .set(INLINE_GUTTER_COLS + INLINE_WRAP_RIGHT_MARGIN + 8);
        assert_eq!(inline_content_width(s.body_width.get()), 8);
        let rows = s.inline_physical_rows();
        // header + ctx(1 row) + long(16 cols → 2 rows).
        assert_eq!(
            rows,
            vec![
                BodyRow::Header,
                BodyRow::Line {
                    sel: 0,
                    cont: false
                },
                BodyRow::Line {
                    sel: 1,
                    cont: false
                },
                BodyRow::Line { sel: 1, cont: true },
            ]
        );
        assert_eq!(s.total_body_rows(), 4);
        // First (non-continuation) physical row of the wrapped line.
        assert_eq!(s.body_row_of(1), 2);
        // A click anywhere on the wrapped line — including its continuation
        // row — selects that diff line; the header row selects nothing.
        assert_eq!(s.selectable_at_body_row(2), Some(1));
        assert_eq!(s.selectable_at_body_row(3), Some(1));
        assert_eq!(s.selectable_at_body_row(0), None);
    }

    #[test]
    fn rendered_inline_rows_match_physical_layout() {
        // The renderer and the row↔line mapping must produce the same number of
        // physical rows, or cursor/scroll/click drift once a line wraps.
        let s = state_with_long_line();
        let width = INLINE_GUTTER_COLS + INLINE_WRAP_RIGHT_MARGIN + 8;
        s.body_width.set(width);
        let pal = Theme::truecolor().review_palette();
        let segs = s.word_segments();
        let lines = review_body_lines(&s, false, &pal, "rs", true, width, &segs, true);
        assert_eq!(lines.len(), s.inline_physical_rows().len());
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
    fn paste_into_draft_appends_text() {
        // Regression: pasting into the review comment box was silently
        // dropped because InputEvent::Paste had no ReviewDiff arm.
        let mut s = state_with_two_files();
        s.focus = ReviewFocus::Body;
        s.begin_comment();
        s.paste_into_draft("see ");
        assert!(s.paste_into_draft("the docs"));
        assert_eq!(s.comment.as_ref().unwrap().input.value(), "see the docs");
    }

    #[test]
    fn paste_into_draft_keeps_newlines_strips_carriage_returns() {
        // Bracketed paste delivers newlines as text, not Enter key events,
        // so a multi-line paste can keep its line breaks (compose_markdown
        // passes them through to the agent verbatim). CRs from CRLF
        // clipboards are dropped.
        let mut s = state_with_two_files();
        s.focus = ReviewFocus::Body;
        s.begin_comment();
        assert!(s.paste_into_draft("let x = 1;\r\nlet y = 2;\r\n"));
        assert_eq!(
            s.comment.as_ref().unwrap().input.value(),
            "let x = 1;\nlet y = 2;\n"
        );
    }

    #[test]
    fn paste_into_draft_without_open_box_is_noop() {
        let mut s = state_with_two_files();
        assert!(!s.paste_into_draft("ignored"));
        assert!(s.comment.is_none());
    }

    #[test]
    fn toggle_layout_flips() {
        let mut s = state_with_two_files();
        assert_eq!(s.layout, ReviewLayout::Inline);
        s.toggle_layout();
        assert_eq!(s.layout, ReviewLayout::SideBySide);
    }

    #[test]
    fn toggle_image_side_flips() {
        let mut s = state_with_two_files();
        assert_eq!(s.image_side, DiffSide::New);
        s.toggle_image_side();
        assert_eq!(s.image_side, DiffSide::Old);
        s.toggle_image_side();
        assert_eq!(s.image_side, DiffSide::New);
    }

    #[test]
    fn shown_image_side_forces_single_sided_statuses() {
        let mut f = file("img.png");
        // Added: always the new side, ignoring the preference.
        f.status = FileStatus::Added;
        assert_eq!(shown_image_side(&f, DiffSide::Old), DiffSide::New);
        // Deleted: always the old side.
        f.status = FileStatus::Deleted;
        assert_eq!(shown_image_side(&f, DiffSide::New), DiffSide::Old);
        // Modified: honours the preference.
        f.status = FileStatus::Modified;
        assert_eq!(shown_image_side(&f, DiffSide::Old), DiffSide::Old);
        assert_eq!(shown_image_side(&f, DiffSide::New), DiffSide::New);
    }

    #[test]
    fn human_size_formats_units() {
        assert_eq!(human_size(None), "? bytes");
        assert_eq!(human_size(Some(512)), "512 bytes");
        assert_eq!(human_size(Some(2048)), "2.0 KiB");
        assert_eq!(human_size(Some(3 * 1024 * 1024)), "3.0 MiB");
    }

    #[test]
    fn image_caption_shows_toggle_hint_only_for_modified() {
        // A modification has two sides, so the `o` toggle is meaningful.
        let modified = image_caption(FileStatus::Modified, "after", Some(2048));
        assert!(
            modified.contains("press o to toggle"),
            "modified caption should advertise the toggle: {modified:?}"
        );
        assert!(modified.contains("after") && modified.contains("2.0 KiB"));

        // Added/deleted images are single-sided; `o` is a no-op, so the hint
        // must not appear (it would be misleading UI).
        let added = image_caption(FileStatus::Added, "after", Some(2048));
        assert!(
            !added.contains("press o to toggle"),
            "added caption must not advertise a no-op toggle: {added:?}"
        );
        let deleted = image_caption(FileStatus::Deleted, "before", Some(512));
        assert!(
            !deleted.contains("press o to toggle"),
            "deleted caption must not advertise a no-op toggle: {deleted:?}"
        );
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
