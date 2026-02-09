//! Preview pane widget
//!
//! Displays captured pane content with scrolling support.

use ansi_to_tui::IntoText;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Text,
    widgets::{Block, Paragraph, ScrollbarState, Widget},
};

/// Preview widget for displaying pane content
pub struct Preview<'a> {
    /// Content to display
    content: &'a str,
    /// Block for borders and title
    block: Option<Block<'a>>,
    /// Scroll offset
    scroll: u16,
}

impl<'a> Preview<'a> {
    /// Create a new preview widget
    pub fn new(content: &'a str) -> Self {
        Self {
            content,
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
}

impl<'a> Widget for Preview<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Convert ANSI escape codes to ratatui styled text
        let text: Text<'_> = self
            .content
            .into_text()
            .unwrap_or_else(|_| Text::raw(self.content));

        // No .wrap() - preserve original formatting (ASCII boxes, tables, etc.)
        let paragraph = Paragraph::new(text).scroll((self.scroll, 0));

        let paragraph = if let Some(block) = self.block {
            paragraph.block(block)
        } else {
            paragraph
        };

        paragraph.render(area, buf);
    }
}

/// Preview state for scrolling
#[derive(Debug)]
pub struct PreviewState {
    /// Current scroll offset (lines from top)
    pub scroll_offset: u16,
    /// Total number of lines in content
    pub total_lines: usize,
    /// Visible height
    pub visible_height: u16,
    /// Whether to follow new content (auto-scroll to bottom)
    follow: bool,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            total_lines: 0,
            visible_height: 0,
            follow: true,
        }
    }
}

impl PreviewState {
    /// Create a new state
    pub fn new() -> Self {
        Self::default()
    }

    /// Update content info
    pub fn set_content(&mut self, content: &str, visible_height: u16) {
        self.total_lines = content.lines().count();
        self.visible_height = visible_height;

        if self.follow {
            self.scroll_to_bottom();
        } else {
            self.clamp_scroll();
        }
    }

    /// Scroll up by n lines
    pub fn scroll_up(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.follow = false;
    }

    /// Scroll down by n lines
    pub fn scroll_down(&mut self, n: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.clamp_scroll();
        // Re-enable follow if we've scrolled to the bottom
        if !self.can_scroll_down() {
            self.follow = true;
        }
    }

    /// Page up
    pub fn page_up(&mut self) {
        let page = self.visible_height.saturating_sub(2);
        self.scroll_up(page);
    }

    /// Page down
    pub fn page_down(&mut self) {
        let page = self.visible_height.saturating_sub(2);
        self.scroll_down(page);
    }

    /// Scroll to top
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.follow = false;
    }

    /// Scroll to bottom
    pub fn scroll_to_bottom(&mut self) {
        self.follow = true;
        if self.total_lines > self.visible_height as usize {
            self.scroll_offset = (self.total_lines - self.visible_height as usize) as u16;
        } else {
            self.scroll_offset = 0;
        }
    }

    /// Ensure scroll offset is within valid range
    fn clamp_scroll(&mut self) {
        let max_scroll = if self.total_lines > self.visible_height as usize {
            (self.total_lines - self.visible_height as usize) as u16
        } else {
            0
        };

        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    /// Get scrollbar state
    pub fn scrollbar_state(&self) -> ScrollbarState {
        ScrollbarState::new(self.total_lines)
            .position(self.scroll_offset as usize)
            .viewport_content_length(self.visible_height as usize)
    }

    /// Check if we can scroll up
    pub fn can_scroll_up(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Check if we can scroll down
    pub fn can_scroll_down(&self) -> bool {
        if self.total_lines <= self.visible_height as usize {
            return false;
        }
        self.scroll_offset < (self.total_lines - self.visible_height as usize) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_state_scrolling() {
        let mut state = PreviewState::new();

        // 100 lines, 20 visible - starts at bottom (follow mode)
        let content = (0..100).map(|i| format!("Line {}", i)).collect::<Vec<_>>().join("\n");
        state.set_content(&content, 20);

        assert_eq!(state.total_lines, 100);
        assert_eq!(state.scroll_offset, 80); // 100 - 20, auto-scrolled to bottom
        assert!(!state.can_scroll_down());
        assert!(state.can_scroll_up());

        // Scroll up disables follow
        state.scroll_up(5);
        assert_eq!(state.scroll_offset, 75);

        // Page up
        state.page_up();
        assert_eq!(state.scroll_offset, 57); // 75 - (20 - 2)

        // Scroll to top
        state.scroll_to_top();
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_preview_state_short_content() {
        let mut state = PreviewState::new();

        // 10 lines, 20 visible - no scrolling needed
        let content = (0..10).map(|i| format!("Line {}", i)).collect::<Vec<_>>().join("\n");
        state.set_content(&content, 20);

        assert!(!state.can_scroll_down());
        assert!(!state.can_scroll_up());

        state.scroll_down(100);
        assert_eq!(state.scroll_offset, 0); // Clamped to 0
    }
}
