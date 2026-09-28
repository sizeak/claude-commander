//! PR badge and pill colour selection, plus the shared PR-pill span builder and
//! OSC 8 hyperlink injection used by both the tree list and the board widget.

use ratatui::{
    buffer::Buffer,
    style::{Color, Modifier, Style},
    text::Span,
};

use crate::theme::Theme;
use claude_commander_core::git::{PrState, effective_pr_state};

/// Does the PR have any label matching the "review needed" list?
pub(super) fn needs_review(labels: &[String], review_labels: &[String]) -> bool {
    !labels.is_empty()
        && labels
            .iter()
            .any(|l| review_labels.iter().any(|r| r.eq_ignore_ascii_case(l)))
}

/// Pick the pill background colour for a PR badge from the same state
/// logic as [`pr_badge_color`], but reading the darker `pr_pill_*_bg`
/// theme fields so bold near-white text remains legible.
pub(crate) fn pr_pill_bg_color(
    theme: &Theme,
    state: Option<PrState>,
    pr_merged: bool,
    is_draft: bool,
    labels: &[String],
    review_labels: &[String],
) -> Color {
    match effective_pr_state(state, pr_merged) {
        PrState::Merged => theme.pr_pill_merged_bg,
        PrState::Closed => theme.pr_pill_closed_bg,
        PrState::Open => {
            if is_draft {
                theme.pr_pill_draft_bg
            } else if needs_review(labels, review_labels) {
                theme.pr_pill_review_bg
            } else {
                theme.pr_pill_open_bg
            }
        }
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
    match effective_pr_state(state, pr_merged) {
        PrState::Merged => theme.status_pr_merged,
        PrState::Closed => theme.pr_closed,
        PrState::Open => {
            if is_draft {
                theme.pr_draft
            } else if needs_review(labels, review_labels) {
                theme.status_pr
            } else {
                theme.pr_open
            }
        }
    }
}

/// Build the spans for a session row's PR badge.
///
/// With `invert_pr_label_color`, renders coloured text on the default
/// background (pre-pill behaviour, a single span). Otherwise renders a
/// non-coloured separator space followed by a padded pill span with a coloured
/// background and bold contrast text. The plain `PR #<n>` text within is what
/// [`inject_pr_hyperlink`] later scans for.
#[allow(clippy::too_many_arguments)] // mirrors the PR field list of the sibling colour fns
pub(crate) fn pr_pill_spans(
    theme: &Theme,
    invert_pr_label_color: bool,
    pr_num: u32,
    state: Option<PrState>,
    pr_merged: bool,
    is_draft: bool,
    labels: &[String],
    review_labels: &[String],
) -> Vec<Span<'static>> {
    if invert_pr_label_color {
        let badge_color = pr_badge_color(theme, state, pr_merged, is_draft, labels, review_labels);
        vec![Span::styled(
            format!(" PR #{}", pr_num),
            Style::default().fg(badge_color),
        )]
    } else {
        let pill_bg = pr_pill_bg_color(theme, state, pr_merged, is_draft, labels, review_labels);
        vec![
            Span::raw(" "),
            Span::styled(
                format!(" PR #{} ", pr_num),
                Style::default()
                    .bg(pill_bg)
                    .fg(theme.pr_pill_text)
                    .add_modifier(Modifier::BOLD),
            ),
        ]
    }
}

/// Scan buffer cells in a row for a matching text string, return starting X position.
pub(crate) fn find_text_in_row(
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

    let width = (x_end.saturating_sub(x_start)) as usize;
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

/// Wrap a single row's `PR #<num>` badge text in OSC 8 hyperlink escape
/// sequences pointing at `url`.
///
/// Each badge character is given its own cell whose symbol is the character
/// wrapped in OSC 8 open/close escapes. The escapes balloon the symbol's
/// computed width far beyond 1, so we pin each cell to
/// [`CellDiffOption::ForcedWidth`] of 1 — otherwise ratatui treats the cell as
/// an enormous multi-width grapheme and blanks every following cell (which
/// silently drops `#<num>` from the badge). Terminals coalesce adjacent cells
/// carrying the same URL into one link. No-op when the badge text isn't found
/// within `[x_start, x_end)` on row `y`.
pub(crate) fn inject_pr_hyperlink(
    buf: &mut Buffer,
    y: u16,
    x_start: u16,
    x_end: u16,
    pr_num: u32,
    url: &str,
) {
    use ratatui::buffer::CellDiffOption;
    use std::num::NonZeroU16;

    let needle = format!("PR #{}", pr_num);
    let Some(start_x) = find_text_in_row(buf, y, x_start, x_end, &needle) else {
        return;
    };

    let one = NonZeroU16::new(1).expect("1 is non-zero");
    let osc_open = format!("\x1B]8;;{}\x07", url);
    let osc_close = "\x1B]8;;\x07";

    for (i, ch) in needle.chars().enumerate() {
        let x = start_x + i as u16;
        if x >= x_end {
            break;
        }
        buf[(x, y)]
            .set_symbol(&format!("{osc_open}{ch}{osc_close}"))
            .set_diff_option(CellDiffOption::ForcedWidth(one));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> Theme {
        Theme::basic()
    }

    /// `Theme::basic()` uses concrete ANSI colours for every `pr_pill_*_bg`
    /// field, none of which equal `Color::default()` (== `Color::Reset`).
    /// Asserting equality to those specific theme fields kills the
    /// `replace pr_pill_bg_color -> Color with Default::default()` mutant.
    #[test]
    fn pr_pill_bg_color_open_returns_theme_open_bg() {
        let theme = test_theme();
        let color = pr_pill_bg_color(&theme, Some(PrState::Open), false, false, &[], &[]);
        assert_eq!(color, theme.pr_pill_open_bg);
        assert_ne!(color, Color::default());
    }

    #[test]
    fn pr_pill_bg_color_merged_returns_theme_merged_bg() {
        let theme = test_theme();
        let color = pr_pill_bg_color(&theme, Some(PrState::Merged), true, false, &[], &[]);
        assert_eq!(color, theme.pr_pill_merged_bg);
        assert_ne!(color, Color::default());
    }

    #[test]
    fn pr_pill_bg_color_closed_returns_theme_closed_bg() {
        let theme = test_theme();
        let color = pr_pill_bg_color(&theme, Some(PrState::Closed), false, false, &[], &[]);
        assert_eq!(color, theme.pr_pill_closed_bg);
    }

    #[test]
    fn pr_pill_bg_color_draft_returns_theme_draft_bg() {
        let theme = test_theme();
        let color = pr_pill_bg_color(&theme, Some(PrState::Open), false, true, &[], &[]);
        assert_eq!(color, theme.pr_pill_draft_bg);
    }

    #[test]
    fn pr_pill_bg_color_review_returns_theme_review_bg() {
        let theme = test_theme();
        let labels = vec!["needs-review".to_string()];
        let review_labels = vec!["needs-review".to_string()];
        let color = pr_pill_bg_color(
            &theme,
            Some(PrState::Open),
            false,
            false,
            &labels,
            &review_labels,
        );
        assert_eq!(color, theme.pr_pill_review_bg);
    }
}
