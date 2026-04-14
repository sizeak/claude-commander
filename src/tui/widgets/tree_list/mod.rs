//! Hierarchical tree list widget
//!
//! Displays projects and their worktree sessions in an indented list.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, ListItem},
};

use crate::session::{AgentState, SessionListItem, SessionStatus};
use crate::tui::theme::Theme;

pub(crate) mod pr_colors;
mod render;
mod state;

#[cfg(test)]
mod tests;

pub use state::TreeListState;

/// Tree branch prefix for worktree items (7 display columns)
const TREE_INDENT: &str = "   └── ";
/// Display width of `TREE_INDENT` in columns
const TREE_INDENT_WIDTH: usize = 7;
/// Width of the number field when `show_numbers` is enabled.
/// Number + trailing space = TREE_INDENT_WIDTH, keeping alignment consistent.
const NUMBER_WIDTH: usize = TREE_INDENT_WIDTH - 1;

/// Braille spinner frames for the Creating status indicator
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    /// Show sequential numbers instead of tree branch prefixes
    show_numbers: bool,
    /// Tick counter for spinner animation
    tick: u64,
    /// Label names that mark an open PR as awaiting reviewer action.
    review_labels: &'a [String],
    /// When true, render PR labels as colored text on default bg (pre-pill
    /// behavior). When false (default), render as a colored pill block.
    invert_pr_label_color: bool,
}

impl<'a> TreeList<'a> {
    /// Create a new tree list
    pub fn new(items: &'a [SessionListItem], theme: &'a Theme) -> Self {
        Self {
            items,
            theme,
            block: None,
            highlight_style: theme.selection().add_modifier(Modifier::BOLD),
            show_numbers: false,
            tick: 0,
            review_labels: &[],
            invert_pr_label_color: false,
        }
    }

    /// When true, render PR labels as colored text on default bg
    /// (pre-pill behavior). Default false renders them as a colored pill.
    pub fn invert_pr_label_color(mut self, b: bool) -> Self {
        self.invert_pr_label_color = b;
        self
    }

    /// Configure the labels that flag an open PR as awaiting reviewer action.
    pub fn review_labels(mut self, labels: &'a [String]) -> Self {
        self.review_labels = labels;
        self
    }

    /// Set the tick counter for spinner animation
    pub fn tick(mut self, tick: u64) -> Self {
        self.tick = tick;
        self
    }

    /// Set the highlight style
    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight_style = style;
        self
    }

    /// Show sequential numbers instead of tree branch prefixes
    pub fn show_numbers(mut self, show: bool) -> Self {
        self.show_numbers = show;
        self
    }

    /// Pick the single status glyph and colour for a worktree row.
    ///
    /// Priority (first wins):
    /// 1. Creating             → animated spinner
    /// 2. Agent `Working`      → animated spinner
    /// 3. Agent `WaitingForInput` → `?` glyph
    /// 4. `unread`             → `◆` diamond
    /// 5. Running (idle/unknown, no unread) → `●` filled circle
    /// 6. Stopped              → `○` open circle
    fn session_status_glyph(
        &self,
        status: SessionStatus,
        agent_state: Option<AgentState>,
        unread: bool,
    ) -> Option<(String, Color)> {
        if status == SessionStatus::Creating {
            let step = self.tick as usize / 3;
            let frame = SPINNER_FRAMES[step % SPINNER_FRAMES.len()];
            return Some((frame.to_string(), self.theme.status_creating));
        }
        if status == SessionStatus::Running {
            match agent_state {
                Some(AgentState::Working) => {
                    let step = self.tick as usize / 3;
                    let frame = SPINNER_FRAMES[step % SPINNER_FRAMES.len()];
                    let color = self.theme.agent_working.color_for_tick(step as u64);
                    return Some((frame.to_string(), color));
                }
                Some(AgentState::WaitingForInput) => {
                    return Some(("?".to_string(), self.theme.agent_waiting));
                }
                _ => {}
            }
            if unread {
                return Some(("◆".to_string(), self.theme.unread_indicator));
            }
            return Some(("●".to_string(), self.theme.status_running));
        }
        // Stopped
        Some(("○".to_string(), self.theme.status_stopped))
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
}
