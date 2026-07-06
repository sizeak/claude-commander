//! TreeList rendering: StatefulWidget impl, list item construction, hyperlinks.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListState, StatefulWidget},
};

use super::*;

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

impl<'a> TreeList<'a> {
    /// Convert items to list items
    pub(super) fn to_list_items(&self) -> Vec<ListItem<'a>> {
        let show_program = self.show_session_program && self.has_mixed_programs();
        let mut project_index: usize = 0;
        let mut worktree_number: usize = 0;
        let mut current_session_color = self.theme.project_color(0).1;

        self.items
            .iter()
            .map(|item| match item {
                SessionListItem::Project {
                    id,
                    name,
                    main_branch,
                    worktree_count,
                    nested,
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

                    // Project sub-headers nested under a section header are
                    // indented one tree-level deeper so the hierarchy reads
                    // SectionHeader > Project > Worktree.
                    let pad = if *nested { "   " } else { " " };

                    let mut spans: Vec<Span<'static>> = vec![
                        Span::raw(pad),
                        Span::styled(
                            name.clone(),
                            Style::default().fg(proj_color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" [{}]", main_branch),
                            Style::default().fg(self.theme.text_accent),
                        ),
                        Span::styled(count_str, Style::default().fg(self.theme.text_secondary)),
                    ];
                    if self.project_is_pull_blocked(id) {
                        spans.push(Span::styled(
                            " ⚠".to_string(),
                            Style::default().fg(self.theme.agent_waiting),
                        ));
                    }

                    ListItem::new(Line::from(spans))
                }
                SessionListItem::Spacer => {
                    let rule: String = "─".repeat(20);
                    let line = Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            rule,
                            Style::default()
                                .fg(self.theme.text_secondary)
                                .add_modifier(Modifier::DIM),
                        ),
                    ]);
                    ListItem::new(line)
                }

                SessionListItem::Worktree {
                    id,
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
                    keep_alive,
                    lfs_pulling,
                    stacked_child,
                    ..
                } => {
                    worktree_number += 1;

                    // Right-aligned session number prefix, with an extra
                    // indent for stacked children so they sit one level deeper
                    // than their stack base.
                    let stack_prefix = if *stacked_child { STACK_INDENT } else { "" };
                    let indent_span = Span::styled(
                        format!(
                            "{stack_prefix}{:>width$} ",
                            worktree_number,
                            width = NUMBER_WIDTH
                        ),
                        Style::default().fg(self.theme.text_secondary),
                    );
                    let mut spans = vec![indent_span];

                    // Single status glyph: spinner > waiting > unread > running > stopped
                    if let Some((glyph, color)) =
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
                    if self.session_has_comments(id) {
                        spans.push(Span::styled(
                            format!(" {COMMENT_MARKER}"),
                            Style::default().fg(self.theme.diff_file_header),
                        ));
                    }
                    if *keep_alive {
                        // Anchored: opted out of auto-hibernation.
                        spans.push(Span::styled(
                            format!(" {KEEP_ALIVE_MARKER}"),
                            Style::default().fg(self.theme.text_accent),
                        ));
                    }
                    if let Some(shown_branch) = crate::session::display_branch(title, branch) {
                        spans.push(Span::styled(
                            format!(" [{}]", shown_branch),
                            Style::default().fg(self.theme.text_accent),
                        ));
                    }

                    if let Some(pr_num) = pr_number {
                        if self.invert_pr_label_color {
                            // Pre-pill behavior: colored text on default bg.
                            let badge_color = pr_colors::pr_badge_color(
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
                        } else {
                            // Pill: non-colored separator space, then a pill
                            // with internal padding, colored bg, and bold
                            // contrast text so it stands out from the row.
                            let pill_bg = pr_colors::pr_pill_bg_color(
                                self.theme,
                                *pr_state,
                                *pr_merged,
                                *pr_draft,
                                pr_labels,
                                self.review_labels,
                            );
                            spans.push(Span::raw(" "));
                            spans.push(Span::styled(
                                format!(" PR #{} ", pr_num),
                                Style::default()
                                    .bg(pill_bg)
                                    .fg(self.theme.pr_pill_text)
                                    .add_modifier(Modifier::BOLD),
                            ));
                        }
                    }

                    if show_program {
                        spans.push(Span::raw(" "));
                        spans.push(Span::styled(
                            format!("({})", program_name(program)),
                            Style::default().fg(self.theme.text_secondary),
                        ));
                    }

                    if *lfs_pulling {
                        spans.push(Span::styled(
                            " ⇣ LFS",
                            Style::default()
                                .fg(self.theme.text_secondary)
                                .add_modifier(Modifier::DIM),
                        ));
                    }

                    let line = Line::from(spans);

                    ListItem::new(line)
                }
                SessionListItem::ServerHeader {
                    name, connection, ..
                } => {
                    use crate::backend::ConnectionState;
                    // A filled dot coloured by health, the server name, and a
                    // short status note. Degraded greys the name and shows the
                    // reason so a down server reads as inert, not active.
                    let (dot_color, name_style, note) = match connection {
                        ConnectionState::Connected => (
                            self.theme.status_running,
                            Style::default()
                                .fg(self.theme.text_primary)
                                .add_modifier(Modifier::BOLD),
                            None,
                        ),
                        ConnectionState::Connecting => (
                            self.theme.text_secondary,
                            Style::default()
                                .fg(self.theme.text_secondary)
                                .add_modifier(Modifier::BOLD),
                            Some(("connecting…".to_string(), self.theme.text_secondary)),
                        ),
                        ConnectionState::Degraded { reason } => (
                            self.theme.modal_warning,
                            Style::default()
                                .fg(self.theme.text_secondary)
                                .add_modifier(Modifier::BOLD | Modifier::DIM),
                            Some((reason.clone(), self.theme.modal_warning)),
                        ),
                    };
                    let mut spans: Vec<Span<'static>> = vec![
                        Span::styled("● ", Style::default().fg(dot_color)),
                        Span::styled(name.clone(), name_style),
                    ];
                    if let Some((text, color)) = note {
                        spans.push(Span::styled(
                            format!(" ({text})"),
                            Style::default().fg(color),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                }
                SessionListItem::SectionHeader {
                    name,
                    count,
                    collapsed,
                    max_sessions,
                } => {
                    let twistie = if *collapsed { "▸ " } else { "▾ " };
                    let (count_text, count_color) = match max_sessions {
                        Some(limit) => {
                            let limit_usize = *limit as usize;
                            let color = if *count > limit_usize {
                                self.theme.modal_error
                            } else if *count == limit_usize {
                                self.theme.modal_warning
                            } else {
                                self.theme.text_secondary
                            };
                            (format!(" ({}/{})", count, limit), color)
                        }
                        None => (format!(" ({})", count), self.theme.text_secondary),
                    };
                    let line = Line::from(vec![
                        Span::raw(" "),
                        Span::styled(twistie, Style::default().fg(self.theme.text_secondary)),
                        Span::styled(
                            name.clone(),
                            Style::default()
                                .fg(self.theme.text_accent)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(count_text, Style::default().fg(count_color)),
                    ]);
                    ListItem::new(line)
                }
            })
            .collect()
    }
}

/// Scan buffer cells in a row for a matching text string, return starting X position.
pub(super) fn find_text_in_row(
    buf: &Buffer,
    y: u16,
    x_start: u16,
    x_end: u16,
    needle: &str,
) -> Option<u16> {
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
/// Each badge character is given its own cell whose symbol is the character
/// wrapped in OSC 8 open/close escapes. The escapes balloon the symbol's
/// computed width far beyond 1, so we pin each cell to [`CellDiffOption::ForcedWidth`]
/// of 1 — otherwise ratatui treats the cell as an enormous multi-width grapheme
/// and blanks every following cell (which silently drops `#<num>` from the
/// badge). Terminals coalesce adjacent cells carrying the same URL into one link.
pub(super) fn inject_pr_hyperlinks(
    list_area: Rect,
    buf: &mut Buffer,
    pr_data: &[Option<(u32, String)>],
    state: &ListState,
) {
    use ratatui::buffer::CellDiffOption;
    use std::num::NonZeroU16;

    let offset = state.offset();
    let visible_rows = list_area.height as usize;
    let one = NonZeroU16::new(1).expect("1 is non-zero");

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

        let osc_open = format!("\x1B]8;;{}\x07", url);
        let osc_close = "\x1B]8;;\x07";

        for (i, ch) in needle.chars().enumerate() {
            let x = start_x + i as u16;
            if x >= list_area.x + list_area.width {
                break;
            }
            buf[(x, y)]
                .set_symbol(&format!("{osc_open}{ch}{osc_close}"))
                .set_diff_option(CellDiffOption::ForcedWidth(one));
        }
    }
}
