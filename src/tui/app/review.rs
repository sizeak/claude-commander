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
use crate::git::{DiffLine, FileDiff, FileStatus, LineOrigin, ParsedDiff};

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
}

impl DiffReviewState {
    pub fn new(
        session_id: SessionId,
        title: String,
        base: String,
        diff: ParsedDiff,
        annotations: Vec<Annotation>,
    ) -> Self {
        Self {
            session_id,
            title,
            base,
            diff,
            annotations,
            selected_file: 0,
            scroll: 0,
            focus: ReviewFocus::FileList,
            cursor: 0,
            visual_anchor: None,
            comment: None,
        }
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

    pub fn next_file(&mut self) {
        if self.diff.files.is_empty() {
            return;
        }
        self.selected_file = (self.selected_file + 1).min(self.diff.files.len() - 1);
        self.reset_file_view();
    }

    pub fn prev_file(&mut self) {
        self.selected_file = self.selected_file.saturating_sub(1);
        self.reset_file_view();
    }

    fn reset_file_view(&mut self) {
        self.scroll = 0;
        self.cursor = 0;
        self.visual_anchor = None;
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
                ReviewFocus::FileList => state.next_file(),
                ReviewFocus::Body => state.move_cursor(true),
            },
            KeyCode::Up | KeyCode::Char('k') => match state.focus {
                ReviewFocus::FileList => state.prev_file(),
                ReviewFocus::Body => state.move_cursor(false),
            },
            KeyCode::Char('v') if state.focus == ReviewFocus::Body => state.toggle_visual(),
            KeyCode::Enter if state.focus == ReviewFocus::Body && state.selectable_count() > 0 => {
                state.comment = Some(CommentDraft {
                    text: String::new(),
                    range: state.selection(),
                });
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
            " ↑↓ extend · Enter comment · v/Esc cancel selection "
        } else {
            " ↑↓/jk move · v select · Enter comment · d delete · a apply · Tab focus · Esc close "
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

        let mut lines: Vec<Line> = Vec::with_capacity(state.diff.files.len());
        for (i, file) in state.diff.files.iter().enumerate() {
            let path = file.display_path().to_string();
            let stat = format!("+{} -{}", file.added, file.removed);
            let count = state.annotation_count(file.display_path());
            let badge = if count > 0 {
                format!(" ✎{count}")
            } else {
                String::new()
            };
            let marker = file_status_marker(file.status);
            let text = format!("{marker} {path}  {stat}{badge}");
            let style = if i == state.selected_file {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(text, style)));
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(format!(" Files ({}) ", state.diff.files.len()));
        frame.render_widget(Paragraph::new(lines).block(block), area);
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

        let lines = review_body_lines(state, focused);

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

/// One-character marker for a file's change status.
fn file_status_marker(status: FileStatus) -> char {
    match status {
        FileStatus::Added => 'A',
        FileStatus::Deleted => 'D',
        FileStatus::Modified => 'M',
        FileStatus::Renamed => 'R',
    }
}

/// Build the inline-rendered body for the current file: hunk headers plus each
/// diff line with an annotation marker, old/new line-number gutter, and +/-
/// colouring. Selected lines (cursor or visual range) are highlighted when the
/// body is focused.
fn review_body_lines(state: &DiffReviewState, focused: bool) -> Vec<Line<'static>> {
    let Some(file) = state.current_file() else {
        return Vec::new();
    };
    let (sel_lo, sel_hi) = state.selection();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut idx = 0; // selectable-line index

    for hunk in &file.hunks {
        let header = format!(
            "@@ -{},{} +{},{} @@ {}",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines, hunk.header
        );
        lines.push(Line::from(Span::styled(
            header,
            Style::default().fg(Color::Cyan),
        )));

        for line in &hunk.lines {
            let old = line
                .old_lineno
                .map(|n| format!("{n:>4}"))
                .unwrap_or_else(|| "    ".to_string());
            let new = line
                .new_lineno
                .map(|n| format!("{n:>4}"))
                .unwrap_or_else(|| "    ".to_string());
            let (marker, color) = match line.origin {
                LineOrigin::Addition => ('+', Color::Green),
                LineOrigin::Deletion => ('-', Color::Red),
                LineOrigin::Context => (' ', Color::Gray),
            };
            // Annotation gutter: ⚠ drifted, ✎ staged, space otherwise.
            let ann = match state.annotation_marker(idx) {
                Some(true) => '⚠',
                Some(false) => '✎',
                None => ' ',
            };
            let text = format!("{ann}{old} {new} │{marker}{}", line.content);

            let mut style = Style::default().fg(color);
            if focused && idx >= sel_lo && idx <= sel_hi {
                style = style.add_modifier(Modifier::REVERSED);
            }
            lines.push(Line::from(Span::styled(text, style)));
            idx += 1;
        }
    }
    lines
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
}
