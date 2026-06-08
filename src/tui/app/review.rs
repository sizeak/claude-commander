//! Full-screen review-diff-and-annotate view.
//!
//! Presentation only: [`DiffReviewState`] holds what's on screen and is opened
//! via `CommanderService::open_review`. The view is hosted as a maximised
//! modal (`Modal::ReviewDiff`); all diff composition, parsing, and annotation
//! logic lives in the library.

use super::*;
use crossterm::event::KeyEvent;

use crate::annotation::{Annotation, AnnotationStatus};
use crate::git::{FileStatus, LineOrigin, ParsedDiff};

/// Which column has keyboard focus inside the review view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFocus {
    FileList,
    Body,
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
        }
    }

    fn current_file(&self) -> Option<&crate::git::FileDiff> {
        self.diff.files.get(self.selected_file)
    }

    /// Number of not-yet-applied annotations anchored to `file`.
    fn annotation_count(&self, file: &str) -> usize {
        self.annotations
            .iter()
            .filter(|a| a.status != AnnotationStatus::Applied && a.file == file)
            .count()
    }

    /// Move to the next/previous file, resetting body scroll.
    pub fn next_file(&mut self) {
        if self.diff.files.is_empty() {
            return;
        }
        self.selected_file = (self.selected_file + 1).min(self.diff.files.len() - 1);
        self.scroll = 0;
    }

    pub fn prev_file(&mut self) {
        self.selected_file = self.selected_file.saturating_sub(1);
        self.scroll = 0;
    }

    pub fn scroll_down(&mut self) {
        let max = self.body_len().saturating_sub(1) as u16;
        self.scroll = (self.scroll + 1).min(max);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            ReviewFocus::FileList => ReviewFocus::Body,
            ReviewFocus::Body => ReviewFocus::FileList,
        };
    }

    /// Rendered body row count for the current file (hunk headers + lines).
    fn body_len(&self) -> usize {
        self.current_file()
            .map(|f| {
                f.hunks
                    .iter()
                    .map(|h| h.lines.len() + 1) // +1 for the hunk header row
                    .sum()
            })
            .unwrap_or(0)
    }
}

impl App {
    /// Open the review view for the selected session.
    pub(super) async fn handle_open_review(&mut self) {
        let Some(session_id) = self.ui_state.selected_session_id else {
            self.ui_state.status_message = Some((
                "Select a session first".to_string(),
                Instant::now() + Duration::from_secs(3),
            ));
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
                    self.ui_state.status_message = Some((
                        "No changes to review".to_string(),
                        Instant::now() + Duration::from_secs(3),
                    ));
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

    /// Handle a key while the review view is open. `state` has been moved out
    /// of `self.ui_state.modal`; it is put back unless the view is closed.
    pub(super) async fn handle_review_key(
        &mut self,
        key: KeyEvent,
        mut state: Box<DiffReviewState>,
    ) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => return, // modal already replaced with None
            KeyCode::Tab => state.toggle_focus(),
            KeyCode::Char(']') => state.next_file(),
            KeyCode::Char('[') => state.prev_file(),
            KeyCode::Down | KeyCode::Char('j') => match state.focus {
                ReviewFocus::FileList => state.next_file(),
                ReviewFocus::Body => state.scroll_down(),
            },
            KeyCode::Up | KeyCode::Char('k') => match state.focus {
                ReviewFocus::FileList => state.prev_file(),
                ReviewFocus::Body => state.scroll_up(),
            },
            _ => {}
        }
        self.ui_state.modal = Modal::ReviewDiff(state);
    }

    /// Render the full-screen review view.
    pub(super) fn render_review_modal(
        &self,
        frame: &mut Frame,
        area: Rect,
        state: &DiffReviewState,
    ) {
        frame.render_widget(Clear, area);

        // Body + footer hint.
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

        let hint = Paragraph::new(Line::from(Span::styled(
            " ↑↓/jk scroll · [ ] prev/next file · Tab focus · Esc close ",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(hint, rows[1]);
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

        let lines = state
            .current_file()
            .map(review_body_lines)
            .unwrap_or_default();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(title);
        frame.render_widget(
            Paragraph::new(lines).block(block).scroll((state.scroll, 0)),
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

/// Build the inline-rendered body for a single file: hunk headers plus each
/// diff line with an old/new line-number gutter and +/- colouring.
fn review_body_lines(file: &crate::git::FileDiff) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
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
            let text = format!("{old} {new} │{marker}{}", line.content);
            lines.push(Line::from(Span::styled(text, Style::default().fg(color))));
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
    fn next_prev_file_clamps() {
        let mut s = state_with_two_files();
        assert_eq!(s.selected_file, 0);
        s.prev_file();
        assert_eq!(s.selected_file, 0, "prev at start stays at 0");
        s.next_file();
        assert_eq!(s.selected_file, 1);
        s.next_file();
        assert_eq!(s.selected_file, 1, "next at end stays at last");
    }

    #[test]
    fn changing_file_resets_scroll() {
        let mut s = state_with_two_files();
        s.focus = ReviewFocus::Body;
        s.scroll_down();
        assert!(s.scroll > 0);
        s.next_file();
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn scroll_clamps_to_body_length() {
        let mut s = state_with_two_files();
        for _ in 0..100 {
            s.scroll_down();
        }
        // file a.rs body: 1 header + 3 lines = 4 rows → max scroll 3.
        assert_eq!(s.scroll, 3);
        s.scroll_up();
        assert_eq!(s.scroll, 2);
    }

    #[test]
    fn toggle_focus_flips() {
        let mut s = state_with_two_files();
        assert_eq!(s.focus, ReviewFocus::FileList);
        s.toggle_focus();
        assert_eq!(s.focus, ReviewFocus::Body);
        s.toggle_focus();
        assert_eq!(s.focus, ReviewFocus::FileList);
    }

    #[test]
    fn annotation_count_ignores_applied_and_other_files() {
        let mut s = state_with_two_files();
        s.annotations.push(Annotation::new(
            "a.rs",
            crate::annotation::AnnotationSide::New,
            (2, 2),
            "let y = 3;",
            "note",
        ));
        let mut applied = Annotation::new(
            "a.rs",
            crate::annotation::AnnotationSide::New,
            (1, 1),
            "fn main() {",
            "old",
        );
        applied.status = AnnotationStatus::Applied;
        s.annotations.push(applied);

        assert_eq!(s.annotation_count("a.rs"), 1);
        assert_eq!(s.annotation_count("b.rs"), 0);
    }
}
