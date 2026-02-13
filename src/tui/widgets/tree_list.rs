//! Hierarchical tree list widget
//!
//! Displays projects and their worktree sessions in an indented list.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, StatefulWidget},
};

use crate::session::{SessionListItem, SessionStatus};
use crate::tui::theme::Theme;

/// Tree list widget for displaying hierarchical sessions
pub struct TreeList<'a> {
    /// Items to display
    items: &'a [SessionListItem],
    /// Theme for styling
    theme: &'a Theme,
    /// Block for borders and title
    block: Option<Block<'a>>,
    /// Style for selected item
    highlight_style: Style,
}

impl<'a> TreeList<'a> {
    /// Create a new tree list
    pub fn new(items: &'a [SessionListItem], theme: &'a Theme) -> Self {
        Self {
            items,
            theme,
            block: None,
            highlight_style: theme.selection().add_modifier(Modifier::BOLD),
        }
    }

    /// Set the block
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Set the highlight style
    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight_style = style;
        self
    }


    /// Check whether sessions use more than one distinct program
    fn has_mixed_programs(&self) -> bool {
        let mut first = None;
        for item in self.items {
            if let SessionListItem::Worktree { program, .. } = item {
                match first {
                    None => first = Some(program.as_str()),
                    Some(p) if p != program => return true,
                    _ => {}
                }
            }
        }
        false
    }

    /// Convert items to list items
    fn to_list_items(&self) -> Vec<ListItem<'a>> {
        let show_program = self.has_mixed_programs();

        self.items
            .iter()
            .map(|item| match item {
                SessionListItem::Project {
                    name,
                    main_branch,
                    worktree_count,
                    ..
                } => {
                    let icon = "📁";
                    let count_str = if *worktree_count > 0 {
                        format!(" ({})", worktree_count)
                    } else {
                        String::new()
                    };

                    let line = Line::from(vec![
                        Span::raw(format!("{} ", icon)),
                        Span::styled(
                            name.clone(),
                            Style::default()
                                .fg(self.theme.text_project)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" [{}]", main_branch),
                            Style::default().fg(self.theme.text_accent),
                        ),
                        Span::styled(count_str, Style::default().fg(self.theme.text_secondary)),
                    ]);

                    ListItem::new(line)
                }

                SessionListItem::Worktree {
                    title,
                    branch,
                    status,
                    program,
                    pr_number,
                    ..
                } => {
                    let (status_icon, status_color) = match status {
                        SessionStatus::Running => ("●", self.theme.status_running),
                        SessionStatus::Paused => ("◐", self.theme.status_paused),
                        SessionStatus::Stopped => ("○", self.theme.status_stopped),
                    };

                    let mut spans = vec![
                        // Indentation for worktrees
                        Span::raw("   └── "),
                        Span::styled(format!("{} ", status_icon), Style::default().fg(status_color)),
                        Span::raw(title.clone()),
                        Span::styled(
                            format!(" [{}]", branch),
                            Style::default().fg(self.theme.text_accent),
                        ),
                    ];

                    if let Some(pr_num) = pr_number {
                        spans.push(Span::styled(
                            format!(" PR #{}", pr_num),
                            Style::default().fg(self.theme.text_pr),
                        ));
                    }

                    if show_program {
                        spans.push(Span::raw(" "));
                        spans.push(Span::styled(
                            format!("({})", program),
                            Style::default().fg(self.theme.text_secondary),
                        ));
                    }

                    let line = Line::from(spans);

                    ListItem::new(line)
                }
            })
            .collect()
    }
}

impl<'a> StatefulWidget for TreeList<'a> {
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Collect PR data before self is consumed
        let pr_data: Vec<Option<(u32, String)>> = self
            .items
            .iter()
            .map(|item| match item {
                SessionListItem::Worktree {
                    pr_number: Some(n),
                    pr_url: Some(url),
                    ..
                } => Some((*n, url.clone())),
                _ => None,
            })
            .collect();

        // Compute inner area before block is moved
        let list_area = self
            .block
            .as_ref()
            .map_or(area, |b| b.inner(area));

        let items = self.to_list_items();
        let list = List::new(items).highlight_style(self.highlight_style);
        let list = if let Some(block) = self.block {
            list.block(block)
        } else {
            list
        };

        StatefulWidget::render(list, area, buf, state);

        // Post-process: inject OSC 8 hyperlinks for PR badges
        inject_pr_hyperlinks(list_area, buf, &pr_data, state);
    }
}

/// Scan buffer cells in a row for a matching text string, return starting X position.
fn find_text_in_row(buf: &Buffer, y: u16, x_start: u16, x_end: u16, needle: &str) -> Option<u16> {
    let chars: Vec<char> = needle.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let width = (x_end - x_start) as usize;
    if width < chars.len() {
        return None;
    }

    // Collect symbols from buffer cells in this row
    let mut row_chars: Vec<(u16, char)> = Vec::new();
    for x in x_start..x_end {
        let cell = &buf[(x, y)];
        let sym = cell.symbol();
        for c in sym.chars() {
            row_chars.push((x, c));
        }
    }

    // Search for needle in row_chars
    'outer: for i in 0..row_chars.len().saturating_sub(chars.len() - 1) {
        for (j, &needle_char) in chars.iter().enumerate() {
            if row_chars[i + j].1 != needle_char {
                continue 'outer;
            }
        }
        return Some(row_chars[i].0);
    }

    None
}

/// Post-process buffer to wrap PR badge text in OSC 8 hyperlink escape sequences.
///
/// Uses 2-char chunking to work around terminal width calculation issues,
/// following ratatui's official hyperlink example pattern.
fn inject_pr_hyperlinks(
    list_area: Rect,
    buf: &mut Buffer,
    pr_data: &[Option<(u32, String)>],
    state: &ListState,
) {
    let offset = state.offset();
    let visible_rows = list_area.height as usize;

    for row in 0..visible_rows {
        let item_idx = offset + row;
        if item_idx >= pr_data.len() {
            break;
        }

        let Some((pr_num, ref url)) = pr_data[item_idx] else {
            continue;
        };

        let y = list_area.y + row as u16;
        let needle = format!("PR #{}", pr_num);

        let Some(start_x) = find_text_in_row(buf, y, list_area.x, list_area.x + list_area.width, &needle) else {
            continue;
        };

        // Apply OSC 8 hyperlink via 2-char chunking
        let osc_open = format!("\x1B]8;;{}\x07", url);
        let osc_close = "\x1B]8;;\x07";

        let needle_chars: Vec<char> = needle.chars().collect();
        let mut char_idx = 0;

        while char_idx < needle_chars.len() {
            let x = start_x + char_idx as u16;
            if x >= list_area.x + list_area.width {
                break;
            }

            // Collect up to 2 characters for this chunk
            let chunk_end = (char_idx + 2).min(needle_chars.len());
            let chunk: String = needle_chars[char_idx..chunk_end].iter().collect();
            let chunk_len = chunk_end - char_idx;

            buf[(x, y)].set_symbol(&format!("{}{}{}", osc_open, chunk, osc_close));

            // If we packed 2 chars into one cell, blank the next cell
            if chunk_len == 2 && x + 1 < list_area.x + list_area.width {
                buf[(x + 1, y)].set_symbol("");
            }

            char_idx = chunk_end;
        }
    }
}

/// Tree list state
#[derive(Debug, Default)]
pub struct TreeListState {
    /// Inner list state
    pub list_state: ListState,
    /// Total number of items
    pub item_count: usize,
}

impl TreeListState {
    /// Create a new state
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the selected index
    pub fn selected(&self) -> Option<usize> {
        self.list_state.selected()
    }

    /// Select an item
    pub fn select(&mut self, index: Option<usize>) {
        self.list_state.select(index);
    }

    /// Select the next item
    pub fn next(&mut self) {
        if self.item_count == 0 {
            return;
        }

        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.item_count - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };

        self.list_state.select(Some(i));
    }

    /// Select the previous item
    pub fn previous(&mut self) {
        if self.item_count == 0 {
            return;
        }

        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.item_count - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };

        self.list_state.select(Some(i));
    }

    /// Update item count and ensure selection is valid
    pub fn set_item_count(&mut self, count: usize) {
        self.item_count = count;

        // Ensure selection is still valid
        if let Some(selected) = self.list_state.selected() {
            if selected >= count && count > 0 {
                self.list_state.select(Some(count - 1));
            } else if count == 0 {
                self.list_state.select(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_list_state_navigation() {
        let mut state = TreeListState::new();
        state.set_item_count(3);

        assert_eq!(state.selected(), None);

        state.next();
        assert_eq!(state.selected(), Some(0));

        state.next();
        assert_eq!(state.selected(), Some(1));

        state.next();
        assert_eq!(state.selected(), Some(2));

        // Wrap around
        state.next();
        assert_eq!(state.selected(), Some(0));

        // Previous
        state.previous();
        assert_eq!(state.selected(), Some(2));
    }

    #[test]
    fn test_tree_list_state_empty() {
        let mut state = TreeListState::new();
        state.set_item_count(0);

        state.next();
        assert_eq!(state.selected(), None);

        state.previous();
        assert_eq!(state.selected(), None);
    }
}
