//! Hierarchical tree list widget
//!
//! Displays projects and their worktree sessions in an indented list.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, StatefulWidget, Widget},
};

use crate::session::{AgentState, SessionListItem, SessionStatus};

/// Tree list widget for displaying hierarchical sessions
pub struct TreeList<'a> {
    /// Items to display
    items: &'a [SessionListItem],
    /// Block for borders and title
    block: Option<Block<'a>>,
    /// Style for selected item
    highlight_style: Style,
    /// Symbol for selected item
    highlight_symbol: &'a str,
}

impl<'a> TreeList<'a> {
    /// Create a new tree list
    pub fn new(items: &'a [SessionListItem]) -> Self {
        Self {
            items,
            block: None,
            highlight_style: Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
            highlight_symbol: "▶ ",
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

    /// Set the highlight symbol
    pub fn highlight_symbol(mut self, symbol: &'a str) -> Self {
        self.highlight_symbol = symbol;
        self
    }

    /// Convert items to list items
    fn to_list_items(&self) -> Vec<ListItem<'a>> {
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
                        Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                        Span::styled(
                            format!(" ({})", main_branch),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(count_str, Style::default().fg(Color::Cyan)),
                    ]);

                    ListItem::new(line)
                }

                SessionListItem::Worktree {
                    title,
                    branch,
                    status,
                    agent_state,
                    program,
                    ..
                } => {
                    let (status_icon, status_color) = match status {
                        SessionStatus::Running => ("●", Color::Green),
                        SessionStatus::Paused => ("◐", Color::Yellow),
                        SessionStatus::Stopped => ("○", Color::DarkGray),
                    };

                    let agent_icon = match agent_state {
                        AgentState::WaitingForInput => "⏳",
                        AgentState::Processing => "⚙️",
                        AgentState::Error => "❌",
                        AgentState::Unknown => "❓",
                    };

                    let line = Line::from(vec![
                        // Indentation for worktrees
                        Span::raw("   └── "),
                        Span::styled(format!("{} ", status_icon), Style::default().fg(status_color)),
                        Span::raw(title.clone()),
                        Span::styled(
                            format!(" [{}]", branch),
                            Style::default().fg(Color::Blue),
                        ),
                        Span::raw(format!(" {} ", agent_icon)),
                        Span::styled(
                            format!("({})", program),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]);

                    ListItem::new(line)
                }
            })
            .collect()
    }
}

impl<'a> StatefulWidget for TreeList<'a> {
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let items = self.to_list_items();

        let list = List::new(items)
            .highlight_style(self.highlight_style)
            .highlight_symbol(self.highlight_symbol);

        let list = if let Some(block) = self.block {
            list.block(block)
        } else {
            list
        };

        StatefulWidget::render(list, area, buf, state);
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
