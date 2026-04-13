//! Hierarchical tree list widget
//!
//! Displays projects and their worktree sessions in an indented list.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, StatefulWidget},
};

use crate::git::PrState;
use crate::session::{AgentState, SessionListItem, SessionStatus};
use crate::tui::theme::Theme;

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
    /// Whether to show status indicator circles (●/◐/○)
    show_status_indicator: bool,
    /// Label names that mark an open PR as awaiting reviewer action.
    review_labels: &'a [String],
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
            show_status_indicator: true,
            review_labels: &[],
        }
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

    /// Set whether to show status indicator circles
    pub fn show_status_indicator(mut self, show: bool) -> Self {
        self.show_status_indicator = show;
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

    /// Convert items to list items
    fn to_list_items(&self) -> Vec<ListItem<'a>> {
        let show_program = self.has_mixed_programs();
        let mut project_index: usize = 0;
        let mut worktree_number: usize = 0;
        let mut current_session_color = self.theme.project_color(0).1;

        self.items
            .iter()
            .map(|item| match item {
                SessionListItem::Project {
                    name,
                    main_branch,
                    worktree_count,
                    ..
                } => {
                    let (proj_color, sess_color) = self.theme.project_color(project_index);
                    current_session_color = sess_color;
                    project_index += 1;

                    let count_str = if *worktree_count > 0 {
                        format!(" ({})", worktree_count)
                    } else {
                        String::new()
                    };

                    let line = Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            name.clone(),
                            Style::default().fg(proj_color).add_modifier(Modifier::BOLD),
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
                    pr_merged,
                    pr_state,
                    pr_draft,
                    pr_labels,
                    agent_state,
                    unread,
                    ..
                } => {
                    worktree_number += 1;

                    let mut spans = vec![
                        // Indentation or number prefix for worktrees
                        if self.show_numbers {
                            Span::styled(
                                format!("{:>width$} ", worktree_number, width = NUMBER_WIDTH),
                                Style::default().fg(self.theme.text_secondary),
                            )
                        } else {
                            Span::raw(TREE_INDENT)
                        },
                    ];

                    // Single status glyph: spinner > waiting > unread > running > stopped
                    if self.show_status_indicator
                        && let Some((glyph, color)) =
                            self.session_status_glyph(*status, *agent_state, *unread)
                    {
                        spans.push(Span::styled(
                            format!("{glyph} "),
                            Style::default().fg(color),
                        ));
                    }

                    let title_style = if *unread {
                        Style::default()
                            .fg(current_session_color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(current_session_color)
                    };
                    spans.push(Span::styled(title.clone(), title_style));
                    spans.push(Span::styled(
                        format!(" [{}]", branch),
                        Style::default().fg(self.theme.text_accent),
                    ));

                    if let Some(pr_num) = pr_number {
                        let badge_color = pr_badge_color(
                            self.theme,
                            *pr_state,
                            *pr_merged,
                            *pr_draft,
                            pr_labels,
                            self.review_labels,
                        );
                        spans.push(Span::styled(
                            format!(" PR #{}", pr_num),
                            Style::default().fg(badge_color),
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

/// Pick the PR badge text colour from PR state, draft flag, and label-based
/// review-needed signalling.
///
/// Priority: merged > closed > draft (within open) > review-needed > open.
/// Falls back to `pr_open` when state is unknown but `pr_merged` is false,
/// and `status_pr_merged` when state is unknown but `pr_merged` is true
/// (handles state.json files written before pr_state was added).
pub(crate) fn pr_badge_color(
    theme: &Theme,
    state: Option<PrState>,
    pr_merged: bool,
    is_draft: bool,
    labels: &[String],
    review_labels: &[String],
) -> Color {
    let effective_state = state.unwrap_or(if pr_merged {
        PrState::Merged
    } else {
        PrState::Open
    });

    match effective_state {
        PrState::Merged => theme.status_pr_merged,
        PrState::Closed => theme.pr_closed,
        PrState::Open => {
            if is_draft {
                theme.pr_draft
            } else if !labels.is_empty()
                && labels
                    .iter()
                    .any(|l| review_labels.iter().any(|r| r.eq_ignore_ascii_case(l)))
            {
                theme.status_pr
            } else {
                theme.pr_open
            }
        }
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
        let list_area = self.block.as_ref().map_or(area, |b| b.inner(area));

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

        let Some(start_x) =
            find_text_in_row(buf, y, list_area.x, list_area.x + list_area.width, &needle)
        else {
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
    use crate::session::{ProjectId, SessionId};
    use ratatui::{Terminal, backend::TestBackend};
    use std::path::PathBuf;

    fn make_project(name: &str, count: usize) -> SessionListItem {
        SessionListItem::Project {
            id: ProjectId::new(),
            name: name.to_string(),
            repo_path: PathBuf::from("/tmp/test"),
            main_branch: "main".to_string(),
            worktree_count: count,
        }
    }

    fn make_worktree(title: &str) -> SessionListItem {
        SessionListItem::Worktree {
            id: SessionId::new(),
            project_id: ProjectId::new(),
            title: title.to_string(),
            branch: "feat".to_string(),
            status: SessionStatus::Running,
            program: "claude".to_string(),
            pr_number: None,
            pr_url: None,
            pr_merged: false,
            pr_state: None,
            pr_draft: false,
            pr_labels: Vec::new(),
            worktree_path: PathBuf::from("/tmp/test"),
            created_at: chrono::Utc::now(),
            agent_state: None,
            unread: false,
        }
    }

    /// Render a TreeList to a buffer and return lines as strings
    fn render_tree(
        items: &[SessionListItem],
        show_numbers: bool,
        width: u16,
        height: u16,
    ) -> Vec<String> {
        let theme = Theme::basic();
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let tree = TreeList::new(items, &theme).show_numbers(show_numbers);
                frame.render_stateful_widget(tree, frame.area(), &mut ListState::default());
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn test_to_list_items_without_numbers_uses_tree_branch() {
        let items = vec![make_project("proj", 1), make_worktree("session-a")];
        let lines = render_tree(&items, false, 40, 3);
        // Worktree line should contain tree branch
        assert!(
            lines[1].contains("└──"),
            "Expected tree branch in: {}",
            lines[1]
        );
    }

    #[test]
    fn test_to_list_items_with_numbers_uses_number_prefix() {
        let items = vec![
            make_project("proj", 2),
            make_worktree("session-a"),
            make_worktree("session-b"),
        ];
        let lines = render_tree(&items, true, 40, 4);
        // First worktree starts with right-aligned "1"
        assert!(
            lines[1].trim_start().starts_with("1 "),
            "Expected number prefix in: '{}'",
            lines[1]
        );
        // Second worktree starts with "2"
        assert!(
            lines[2].trim_start().starts_with("2 "),
            "Expected number prefix in: '{}'",
            lines[2]
        );
        // No tree branches
        assert!(
            !lines[1].contains("└──"),
            "Should not have tree branch with numbers"
        );
    }

    #[test]
    fn test_numbers_are_sequential_across_projects() {
        let items = vec![
            make_project("proj-a", 1),
            make_worktree("session-1"),
            make_project("proj-b", 1),
            make_worktree("session-2"),
        ];
        let lines = render_tree(&items, true, 40, 5);
        // Session under proj-a is #1
        assert!(
            lines[1].trim_start().starts_with("1 "),
            "Expected 1 in: '{}'",
            lines[1]
        );
        // Session under proj-b is #2 (not restarting)
        assert!(
            lines[3].trim_start().starts_with("2 "),
            "Expected 2 in: '{}'",
            lines[3]
        );
    }

    #[test]
    fn test_double_digit_number_formatting() {
        let mut items = vec![make_project("proj", 12)];
        for i in 1..=12 {
            items.push(make_worktree(&format!("s-{}", i)));
        }
        let lines = render_tree(&items, true, 40, 14);
        // Single digit right-aligned
        assert!(
            lines[1].trim_start().starts_with("1 "),
            "line 1: '{}'",
            lines[1]
        );
        // Double digit
        assert!(
            lines[10].trim_start().starts_with("10 "),
            "line 10: '{}'",
            lines[10]
        );
        assert!(
            lines[12].trim_start().starts_with("12 "),
            "line 12: '{}'",
            lines[12]
        );
    }

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

    #[test]
    fn test_previous_wraps_to_last() {
        let mut state = TreeListState::new();
        state.set_item_count(5);
        state.select(Some(0));

        state.previous();
        assert_eq!(state.selected(), Some(4));
    }

    #[test]
    fn test_next_wraps_to_first() {
        let mut state = TreeListState::new();
        state.set_item_count(5);
        state.select(Some(4));

        state.next();
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn test_set_item_count_clamps_selection() {
        let mut state = TreeListState::new();
        state.set_item_count(10);
        state.select(Some(7));

        state.set_item_count(5);
        assert_eq!(state.selected(), Some(4));
    }

    #[test]
    fn test_set_item_count_zero_clears_selection() {
        let mut state = TreeListState::new();
        state.set_item_count(5);
        state.select(Some(3));

        state.set_item_count(0);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn test_set_item_count_preserves_valid_selection() {
        let mut state = TreeListState::new();
        state.set_item_count(10);
        state.select(Some(3));

        state.set_item_count(8);
        assert_eq!(state.selected(), Some(3));
    }

    #[test]
    fn test_single_item_navigation() {
        let mut state = TreeListState::new();
        state.set_item_count(1);
        state.select(Some(0));

        state.next();
        assert_eq!(state.selected(), Some(0));

        state.previous();
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn test_next_from_none_selects_first() {
        let mut state = TreeListState::new();
        state.set_item_count(3);
        assert_eq!(state.selected(), None);

        state.next();
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn test_previous_from_none_selects_first() {
        let mut state = TreeListState::new();
        state.set_item_count(3);
        assert_eq!(state.selected(), None);

        state.previous();
        assert_eq!(state.selected(), Some(0));
    }

    // -- pr_badge_color --

    fn review_labels() -> Vec<String> {
        vec![
            "dev-review-required".into(),
            "ready-for-test".into(),
            "trivial".into(),
        ]
    }

    #[test]
    fn test_pr_badge_color_open() {
        let theme = Theme::basic();
        let c = pr_badge_color(
            &theme,
            Some(PrState::Open),
            false,
            false,
            &[],
            &review_labels(),
        );
        assert_eq!(c, theme.pr_open);
    }

    #[test]
    fn test_pr_badge_color_merged() {
        let theme = Theme::basic();
        let c = pr_badge_color(
            &theme,
            Some(PrState::Merged),
            true,
            false,
            &[],
            &review_labels(),
        );
        assert_eq!(c, theme.status_pr_merged);
    }

    #[test]
    fn test_pr_badge_color_closed() {
        let theme = Theme::basic();
        let c = pr_badge_color(
            &theme,
            Some(PrState::Closed),
            false,
            false,
            &[],
            &review_labels(),
        );
        assert_eq!(c, theme.pr_closed);
    }

    #[test]
    fn test_pr_badge_color_draft_takes_precedence_over_label() {
        let theme = Theme::basic();
        let labels = vec!["dev-review-required".into()];
        let c = pr_badge_color(
            &theme,
            Some(PrState::Open),
            false,
            true,
            &labels,
            &review_labels(),
        );
        assert_eq!(c, theme.pr_draft);
    }

    #[test]
    fn test_pr_badge_color_review_label_match() {
        let theme = Theme::basic();
        let labels = vec!["unrelated".into(), "ready-for-test".into()];
        let c = pr_badge_color(
            &theme,
            Some(PrState::Open),
            false,
            false,
            &labels,
            &review_labels(),
        );
        assert_eq!(c, theme.status_pr);
    }

    #[test]
    fn test_pr_badge_color_review_label_case_insensitive() {
        let theme = Theme::basic();
        let labels = vec!["Dev-Review-Required".into()];
        let c = pr_badge_color(
            &theme,
            Some(PrState::Open),
            false,
            false,
            &labels,
            &review_labels(),
        );
        assert_eq!(c, theme.status_pr);
    }

    #[test]
    fn test_pr_badge_color_non_matching_labels_fall_through_to_open() {
        let theme = Theme::basic();
        let labels = vec!["bug".into(), "documentation".into()];
        let c = pr_badge_color(
            &theme,
            Some(PrState::Open),
            false,
            false,
            &labels,
            &review_labels(),
        );
        assert_eq!(c, theme.pr_open);
    }

    #[test]
    fn test_pr_badge_color_unknown_state_uses_pr_merged_flag_for_merged() {
        // Backward compat: pre-pr_state state.json with pr_merged=true
        let theme = Theme::basic();
        let c = pr_badge_color(&theme, None, true, false, &[], &review_labels());
        assert_eq!(c, theme.status_pr_merged);
    }

    #[test]
    fn test_pr_badge_color_unknown_state_falls_back_to_open() {
        // Backward compat: pre-pr_state state.json with pr_merged=false
        let theme = Theme::basic();
        let c = pr_badge_color(&theme, None, false, false, &[], &review_labels());
        assert_eq!(c, theme.pr_open);
    }

    // -- session_status_glyph (single unified icon column) --

    fn empty_items() -> [SessionListItem; 0] {
        []
    }

    fn make_tree<'a>(theme: &'a Theme, items: &'a [SessionListItem]) -> TreeList<'a> {
        TreeList::new(items, theme)
    }

    #[test]
    fn test_glyph_working_shows_spinner() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, c) = tree
            .session_status_glyph(SessionStatus::Running, Some(AgentState::Working), false)
            .unwrap();
        assert!(SPINNER_FRAMES.contains(&g.as_str()));
        // Default theme uses Rainbow → colour comes from the rainbow palette
        assert!(crate::config::theme::RAINBOW_PALETTE.contains(&c));
    }

    #[test]
    fn test_glyph_working_beats_unread() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, _) = tree
            .session_status_glyph(SessionStatus::Running, Some(AgentState::Working), true)
            .unwrap();
        assert!(SPINNER_FRAMES.contains(&g.as_str()));
    }

    #[test]
    fn test_glyph_waiting_for_input() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, c) = tree
            .session_status_glyph(
                SessionStatus::Running,
                Some(AgentState::WaitingForInput),
                false,
            )
            .unwrap();
        assert_eq!(g, "?");
        assert_eq!(c, theme.agent_waiting);
    }

    #[test]
    fn test_glyph_waiting_beats_unread() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, _) = tree
            .session_status_glyph(
                SessionStatus::Running,
                Some(AgentState::WaitingForInput),
                true,
            )
            .unwrap();
        assert_eq!(g, "?");
    }

    #[test]
    fn test_glyph_unread_when_idle() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, c) = tree
            .session_status_glyph(SessionStatus::Running, Some(AgentState::Idle), true)
            .unwrap();
        assert_eq!(g, "◆");
        assert_eq!(c, theme.unread_indicator);
    }

    #[test]
    fn test_glyph_running_idle_no_unread() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, c) = tree
            .session_status_glyph(SessionStatus::Running, Some(AgentState::Idle), false)
            .unwrap();
        assert_eq!(g, "●");
        assert_eq!(c, theme.status_running);
    }

    #[test]
    fn test_glyph_running_unknown_no_unread() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, c) = tree
            .session_status_glyph(SessionStatus::Running, Some(AgentState::Unknown), false)
            .unwrap();
        assert_eq!(g, "●");
        assert_eq!(c, theme.status_running);
    }

    #[test]
    fn test_glyph_running_no_agent_state_no_unread() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, c) = tree
            .session_status_glyph(SessionStatus::Running, None, false)
            .unwrap();
        assert_eq!(g, "●");
        assert_eq!(c, theme.status_running);
    }

    #[test]
    fn test_glyph_stopped() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, c) = tree
            .session_status_glyph(SessionStatus::Stopped, None, false)
            .unwrap();
        assert_eq!(g, "○");
        assert_eq!(c, theme.status_stopped);
    }

    #[test]
    fn test_glyph_stopped_ignores_unread_and_agent_state() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        // Stopped sessions can't have a meaningful agent state, but ensure
        // the glyph is consistent regardless.
        let (g, _) = tree
            .session_status_glyph(SessionStatus::Stopped, Some(AgentState::Working), true)
            .unwrap();
        assert_eq!(g, "○");
    }

    #[test]
    fn test_glyph_creating_shows_spinner() {
        let theme = Theme::basic();
        let items = empty_items();
        let tree = make_tree(&theme, &items);
        let (g, c) = tree
            .session_status_glyph(SessionStatus::Creating, None, false)
            .unwrap();
        assert!(SPINNER_FRAMES.contains(&g.as_str()));
        assert_eq!(c, theme.status_creating);
    }
}
