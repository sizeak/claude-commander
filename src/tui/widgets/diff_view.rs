//! Diff view widget
//!
//! Displays git diff with syntax highlighting for added/removed lines.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

use crate::git::DiffInfo;
use crate::tui::theme::Theme;

/// Diff view widget
pub struct DiffView<'a> {
    /// Diff info to display
    diff_info: &'a DiffInfo,
    /// Theme for styling
    theme: &'a Theme,
    /// Block for borders and title
    block: Option<Block<'a>>,
    /// Scroll offset
    scroll: u16,
}

impl<'a> DiffView<'a> {
    /// Create a new diff view
    pub fn new(diff_info: &'a DiffInfo, theme: &'a Theme) -> Self {
        Self {
            diff_info,
            theme,
            block: None,
            scroll: 0,
        }
    }

    /// Set the block
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Set the scroll offset
    pub fn scroll(mut self, scroll: u16) -> Self {
        self.scroll = scroll;
        self
    }

    /// Style a single diff line
    fn style_line(&self, line: &'a str) -> Line<'a> {
        if line.starts_with('+') && !line.starts_with("+++") {
            Line::from(Span::styled(
                line,
                Style::default().fg(self.theme.diff_added),
            ))
        } else if line.starts_with('-') && !line.starts_with("---") {
            Line::from(Span::styled(
                line,
                Style::default().fg(self.theme.diff_removed),
            ))
        } else if line.starts_with("@@") {
            Line::from(Span::styled(
                line,
                Style::default().fg(self.theme.diff_hunk_header),
            ))
        } else if line.starts_with("diff ") || line.starts_with("index ") {
            Line::from(Span::styled(
                line,
                Style::default()
                    .fg(self.theme.diff_file_header)
                    .add_modifier(Modifier::BOLD),
            ))
        } else if line.starts_with("---") || line.starts_with("+++") {
            Line::from(Span::styled(
                line,
                Style::default().fg(self.theme.diff_file_header),
            ))
        } else {
            Line::from(Span::raw(line))
        }
    }
}

impl<'a> Widget for DiffView<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Compute inner area accounting for block borders
        let inner_height = if self.block.is_some() {
            area.height.saturating_sub(2) as usize
        } else {
            area.height as usize
        };

        let lines: Vec<Line<'a>> = if self.diff_info.diff.is_empty() {
            vec![Line::from(Span::styled(
                "No changes",
                Style::default().fg(self.theme.text_secondary),
            ))]
        } else {
            // Only style the visible window of lines
            self.diff_info
                .diff
                .lines()
                .skip(self.scroll as usize)
                .take(inner_height)
                .map(|line| self.style_line(line))
                .collect()
        };

        // scroll is (0,0) since we already sliced to the visible window
        let paragraph = Paragraph::new(lines);

        let paragraph = if let Some(block) = self.block {
            paragraph.block(block)
        } else {
            paragraph
        };

        paragraph.render(area, buf);
    }
}

/// Diff view state (reuses PreviewState for scrolling)
pub type DiffViewState = super::PreviewState;

/// Summary bar for diff statistics
#[allow(dead_code)]
pub struct DiffSummary<'a> {
    /// Diff info
    diff_info: &'a DiffInfo,
    /// Theme for styling
    theme: &'a Theme,
}

impl<'a> DiffSummary<'a> {
    /// Create a new diff summary
    #[allow(dead_code)]
    pub fn new(diff_info: &'a DiffInfo, theme: &'a Theme) -> Self {
        Self { diff_info, theme }
    }
}

impl<'a> Widget for DiffSummary<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let line = if self.diff_info.has_changes() {
            Line::from(vec![
                Span::styled(
                    format!("{} file(s)", self.diff_info.files_changed),
                    Style::default().fg(Color::White),
                ),
                Span::raw(" | "),
                Span::styled(
                    format!("+{}", self.diff_info.lines_added),
                    Style::default().fg(self.theme.diff_added),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("-{}", self.diff_info.lines_removed),
                    Style::default().fg(self.theme.diff_removed),
                ),
            ])
        } else {
            Line::from(Span::styled(
                "No changes",
                Style::default().fg(self.theme.text_secondary),
            ))
        };

        buf.set_line(area.x, area.y, &line, area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn make_diff_info(diff: &str) -> DiffInfo {
        DiffInfo {
            diff: diff.to_string(),
            files_changed: 1,
            lines_added: 5,
            lines_removed: 3,
            line_count: diff.lines().count(),
            computed_at: Instant::now(),
            base_commit: "abc123".to_string(),
        }
    }

    #[test]
    fn test_diff_view_styling() {
        let diff = r#"diff --git a/file.rs b/file.rs
index abc123..def456 100644
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,5 @@
 context line
-removed line
+added line
+another added
 more context"#;

        let info = make_diff_info(diff);
        let theme = Theme::default();
        let view = DiffView::new(&info, &theme);
        // Style a single line to test
        let styled = view.style_line("+added line");
        assert_eq!(styled.spans.len(), 1);
    }

    #[test]
    fn test_empty_diff() {
        let info = DiffInfo::empty();
        let theme = Theme::default();

        // Render into a buffer to verify it doesn't panic
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let view = DiffView::new(&info, &theme);
        view.render(area, &mut buf);
    }
}
