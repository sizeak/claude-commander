//! Pure board geometry.
//!
//! Terminal-free layout maths for the kanban board so it is unit-testable
//! without rendering: column/​sidebar rectangles, per-card display-line ranges,
//! scroll-into-view, and point→column hit-testing. All functions are total —
//! degenerate sizes (zero width/height, more separators than columns fit)
//! return empty/clamped geometry rather than panicking.

use std::ops::Range;

use ratatui::layout::Rect;

/// Fixed width of the project sidebar, in columns. Clamped to the available
/// width on very narrow terminals.
pub const SIDEBAR_WIDTH: u16 = 24;

/// Resolved board rectangles for one frame.
///
/// The horizontal layout is `[sidebar][│ col 0][│ col 1]…[│ col n-1]` — a
/// one-column separator precedes every section column (including the first,
/// between it and the sidebar), so `separators.len() == columns.len()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRects {
    /// The project sidebar on the left.
    pub sidebar: Rect,
    /// One rectangle per section column, left to right.
    pub columns: Vec<Rect>,
    /// The x position of each thin `│` separator; `separators[i]` sits
    /// immediately left of `columns[i]`.
    pub separators: Vec<u16>,
}

/// Split `area` into the fixed-width sidebar and `n_cols` equal-width section
/// columns separated by single-column dividers.
///
/// The sidebar takes [`SIDEBAR_WIDTH`] (clamped to `area.width`). The remaining
/// width, minus the `n_cols` separator columns, is divided as evenly as
/// possible among the columns (any remainder goes to the leftmost columns, so
/// widths differ by at most one). Degenerate inputs — zero area, or too little
/// width to fit the separators and columns — yield zero-width columns and never
/// panic or produce rectangles extending past `area`.
pub fn column_rects(area: Rect, n_cols: usize) -> BoardRects {
    let right_edge = area.x.saturating_add(area.width);
    let sidebar_w = SIDEBAR_WIDTH.min(area.width);
    let sidebar = Rect {
        x: area.x,
        y: area.y,
        width: sidebar_w,
        height: area.height,
    };

    if n_cols == 0 || area.width == 0 || area.height == 0 {
        return BoardRects {
            sidebar,
            columns: Vec::new(),
            separators: Vec::new(),
        };
    }

    let n = n_cols as u16;
    // Width left of the sidebar, then reserve one column per separator.
    let remaining = area.width.saturating_sub(sidebar_w);
    let for_columns = remaining.saturating_sub(n);
    let base = for_columns / n;
    let extra = for_columns % n;

    let mut columns = Vec::with_capacity(n_cols);
    let mut separators = Vec::with_capacity(n_cols);
    let mut x = sidebar.x.saturating_add(sidebar_w);
    for i in 0..n_cols {
        // Separator, clamped so it never sits past the area.
        let sep_x = x.min(right_edge);
        separators.push(sep_x);
        x = x.saturating_add(1).min(right_edge);

        let mut w = base;
        if (i as u16) < extra {
            w += 1;
        }
        // Never let a column extend past the area's right edge.
        w = w.min(right_edge.saturating_sub(x));
        columns.push(Rect {
            x,
            y: area.y,
            width: w,
            height: area.height,
        });
        x = x.saturating_add(w);
    }

    BoardRects {
        sidebar,
        columns,
        separators,
    }
}

/// Compute the display-line range each card occupies when stacked vertically
/// with no gap between cards.
///
/// A card's height is its row count plus two for the top and bottom borders, so
/// `card_row_counts[i]` rows produce a range of length `rows + 2`. The returned
/// ranges are contiguous starting at line 0.
pub fn card_line_ranges(card_row_counts: &[usize]) -> Vec<Range<usize>> {
    let mut ranges = Vec::with_capacity(card_row_counts.len());
    let mut start = 0;
    for &rows in card_row_counts {
        let height = rows + 2;
        ranges.push(start..start + height);
        start += height;
    }
    ranges
}

/// Minimal scroll offset (in display lines) so the selected row is visible in a
/// `viewport_h`-line viewport.
///
/// When the selected card fits within the viewport, this keeps the whole card
/// visible (card-granular): `scroll` unchanged if it already fits, scrolled up
/// to its first line when above, or down so its last line rests on the bottom
/// edge when below.
///
/// When the card is taller than the viewport it can never be shown whole, so
/// scrolling becomes row-granular: the offset is chosen so the selected row
/// (`row_within_card`, a 0-based interior row) sits strictly between the
/// clipped top and bottom borders of the visible slice, moving as little as
/// possible. This is what lets the cursor reach a middle row of an over-tall
/// card — card-granular scrolling would flip-flop between the card's top and
/// bottom and never reveal the interior.
///
/// Returns `scroll` untouched for an empty range list, an out-of-range card, or
/// a zero-height viewport.
pub fn ensure_visible(
    scroll: usize,
    ranges: &[Range<usize>],
    selected_card: usize,
    row_within_card: usize,
    viewport_h: usize,
) -> usize {
    if viewport_h == 0 {
        return scroll;
    }
    let Some(range) = ranges.get(selected_card) else {
        return scroll;
    };
    let card_h = range.end - range.start;
    if card_h <= viewport_h {
        // Card fits: keep it fully visible (card-granular).
        if range.start < scroll {
            range.start
        } else if range.end > scroll + viewport_h {
            range.end.saturating_sub(viewport_h)
        } else {
            scroll
        }
    } else {
        // Card taller than the viewport: keep the selected row inside the
        // visible box's clipped borders. Its absolute display line is the top
        // border (`range.start`) + 1 + the interior offset. For that line `L`
        // to render (top border < L < bottom border of the visible slice), the
        // scroll must lie in `[L + 2 - viewport_h, L - 1]`; clamp into it,
        // moving minimally.
        let line = range.start + 1 + row_within_card;
        let min_sc = (line + 2).saturating_sub(viewport_h);
        let max_sc = line.saturating_sub(1);
        if min_sc > max_sc {
            // Viewport shorter than 3 lines, so it can't frame the row between a
            // top and bottom border — there is no interior line to land on.
            // Fall back to keeping the selected row itself on-screen with
            // minimal movement (clamp into the range where `line` is visible),
            // which is stable (idempotent) rather than flip-flopping.
            let row_min = (line + 1).saturating_sub(viewport_h);
            scroll.clamp(row_min, line)
        } else {
            scroll.clamp(min_sc, max_sc)
        }
    }
}

/// Map an x coordinate to a board column index: `0` for the sidebar, `i + 1`
/// for `rects.columns[i]`. Returns `None` for x on a separator or outside the
/// board entirely.
pub fn column_at_x(rects: &BoardRects, x: u16) -> Option<usize> {
    if x >= rects.sidebar.x && x < rects.sidebar.x + rects.sidebar.width {
        return Some(0);
    }
    rects
        .columns
        .iter()
        .position(|c| x >= c.x && x < c.x + c.width)
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    // --- column_rects ----------------------------------------------------

    #[test]
    fn column_rects_gives_equal_widths_and_separator_positions() {
        // width 100, sidebar 24, 3 separators → 100 - 24 - 3 = 73 for columns.
        // 73 / 3 = 24 base, remainder 1 → first column gets 25.
        let rects = column_rects(area(100, 40), 3);
        assert_eq!(rects.sidebar.width, SIDEBAR_WIDTH);
        assert_eq!(rects.columns.len(), 3);
        assert_eq!(rects.separators.len(), 3);

        let widths: Vec<u16> = rects.columns.iter().map(|c| c.width).collect();
        assert_eq!(widths, vec![25, 24, 24], "remainder goes to the leftmost");

        // Separators sit immediately left of each column.
        for (sep, col) in rects.separators.iter().zip(&rects.columns) {
            assert_eq!(*sep + 1, col.x);
        }
        // First separator is right after the sidebar.
        assert_eq!(rects.separators[0], rects.sidebar.x + rects.sidebar.width);
        // Columns are contiguous: each starts one past the previous column end.
        assert_eq!(
            rects.columns[1].x,
            rects.columns[0].x + rects.columns[0].width + 1
        );
        assert_eq!(
            rects.columns[2].x,
            rects.columns[1].x + rects.columns[1].width + 1
        );
    }

    #[test]
    fn column_rects_divides_evenly_when_possible() {
        // width 90, sidebar 24, 3 separators → 63 for columns, 63/3 = 21 each.
        let rects = column_rects(area(90, 10), 3);
        let widths: Vec<u16> = rects.columns.iter().map(|c| c.width).collect();
        assert_eq!(widths, vec![21, 21, 21]);
    }

    #[test]
    fn column_rects_full_height_and_no_columns_case() {
        let rects = column_rects(area(80, 30), 0);
        assert_eq!(rects.columns.len(), 0);
        assert_eq!(rects.separators.len(), 0);
        assert_eq!(rects.sidebar.height, 30);
    }

    #[test]
    fn column_rects_clamps_sidebar_on_narrow_terminal() {
        // Terminal narrower than the sidebar: sidebar takes all of it, columns
        // collapse to zero width without panicking or spilling over.
        let rects = column_rects(area(10, 20), 3);
        assert_eq!(rects.sidebar.width, 10);
        let right = 10;
        for col in &rects.columns {
            assert!(col.x <= right, "column x within area");
            assert!(col.x + col.width <= right, "column stays within area");
        }
        for sep in &rects.separators {
            assert!(*sep <= right);
        }
    }

    #[test]
    fn column_rects_zero_size_is_safe() {
        let rects = column_rects(area(0, 0), 4);
        assert_eq!(rects.sidebar.width, 0);
        assert_eq!(rects.columns.len(), 0);
        assert_eq!(rects.separators.len(), 0);
    }

    #[test]
    fn column_rects_more_separators_than_width_is_safe() {
        // width 26: sidebar clamps to 24, leaving 2 for 5 separators + columns.
        let rects = column_rects(area(26, 5), 5);
        assert_eq!(rects.columns.len(), 5);
        let right = 26;
        for col in &rects.columns {
            assert!(col.x + col.width <= right);
        }
        for sep in &rects.separators {
            assert!(*sep <= right);
        }
    }

    // --- card_line_ranges ------------------------------------------------

    #[test]
    fn card_line_ranges_add_two_border_lines_per_card() {
        // rows: 1, 2, 3 → heights 3, 4, 5, stacked contiguously from 0.
        let ranges = card_line_ranges(&[1, 2, 3]);
        assert_eq!(ranges, vec![0..3, 3..7, 7..12]);
    }

    #[test]
    fn card_line_ranges_empty() {
        assert!(card_line_ranges(&[]).is_empty());
    }

    // --- ensure_visible --------------------------------------------------

    #[test]
    fn ensure_visible_scrolls_down_to_reveal_card_below() {
        let ranges = card_line_ranges(&[1, 1, 1, 1]); // 3,3,3,3 → 0..3,3..6,6..9,9..12
        // viewport of 6 lines starting at 0 shows cards 0 and 1; select card 3
        // (9..12) → scroll so 12 rests at the bottom: 12 - 6 = 6.
        assert_eq!(ensure_visible(0, &ranges, 3, 0, 6), 6);
    }

    #[test]
    fn ensure_visible_scrolls_up_to_reveal_card_above() {
        let ranges = card_line_ranges(&[1, 1, 1, 1]);
        // Scrolled to 6; selecting card 0 (0..3) scrolls up to its start.
        assert_eq!(ensure_visible(6, &ranges, 0, 0, 6), 0);
    }

    #[test]
    fn ensure_visible_is_a_noop_when_already_visible() {
        let ranges = card_line_ranges(&[1, 1, 1, 1]);
        // Card 1 (3..6) is fully within [0, 6).
        assert_eq!(ensure_visible(0, &ranges, 1, 0, 6), 0);
    }

    #[test]
    fn ensure_visible_zero_viewport_and_out_of_range_are_safe() {
        let ranges = card_line_ranges(&[1, 1]);
        assert_eq!(
            ensure_visible(2, &ranges, 0, 0, 0),
            2,
            "zero viewport unchanged"
        );
        assert_eq!(
            ensure_visible(2, &ranges, 9, 0, 6),
            2,
            "out-of-range card unchanged"
        );
        assert_eq!(ensure_visible(2, &[], 0, 0, 6), 2, "empty ranges unchanged");
    }

    #[test]
    fn ensure_visible_reveals_middle_row_of_over_tall_card_and_is_stable() {
        // A single 20-row card (height 22) in an 8-line viewport can never be
        // shown whole. Selecting an interior middle row must bring it into view
        // and, crucially, stay put on the next computation (no flip-flop).
        let ranges = card_line_ranges(&[20]); // 0..22
        let viewport_h = 8;
        let row = 9; // interior row 9 → display line 0 + 1 + 9 = 10

        let sc = ensure_visible(0, &ranges, 0, row, viewport_h);
        // Row line 10 must sit strictly inside the visible slice's borders:
        // top border at max(0, sc), bottom border at min(21, sc+7).
        let line = 10;
        let top_border = sc.max(0);
        let bottom_border = (sc + viewport_h - 1).min(21);
        assert!(
            top_border < line && line < bottom_border,
            "row line {line} not inside visible borders ({top_border}, {bottom_border}) at scroll {sc}"
        );

        // Re-running from the resolved scroll is a fixed point.
        assert_eq!(
            ensure_visible(sc, &ranges, 0, row, viewport_h),
            sc,
            "scroll must be stable — the old whole-card logic flip-flopped 0/14"
        );
    }

    #[test]
    fn ensure_visible_over_tall_card_scrolls_minimally_for_top_and_bottom_rows() {
        let ranges = card_line_ranges(&[20]); // 0..22, height 22
        let viewport_h = 8;

        // First interior row (offset 0 → line 1): already visible from the top,
        // so no scroll.
        assert_eq!(ensure_visible(0, &ranges, 0, 0, viewport_h), 0);

        // Last interior row (offset 19 → line 20): must scroll down so the row
        // sits just above the bottom border. Window is [20+2-8, 20-1] = [14,19];
        // clamping 0 into it gives 14.
        assert_eq!(ensure_visible(0, &ranges, 0, 19, viewport_h), 14);
    }

    #[test]
    fn ensure_visible_over_tall_card_keeps_selected_row_on_screen_in_tiny_viewports() {
        // A 20-row card (height 22) can never be shown whole. Selecting interior
        // row 9 (display line 0 + 1 + 9 = 10) in viewports too short to frame the
        // row between two borders must still keep the row itself on-screen, be a
        // fixed point (no flip-flop), and never panic.
        let ranges = card_line_ranges(&[20]); // 0..22
        let line = 10usize;

        // h == 3: the smallest viewport with an interior line. The selected row
        // must land strictly between the clipped top and bottom borders.
        let sc3 = ensure_visible(0, &ranges, 0, 9, 3);
        assert!(
            sc3 < line && line < sc3 + 3 - 1,
            "h=3 must show row {line} as an interior line (scroll {sc3})"
        );
        assert_eq!(ensure_visible(sc3, &ranges, 0, 9, 3), sc3, "h=3 stable");

        // h == 2 and h == 1: no interior line exists, so the row can't be framed;
        // the least-wrong fallback keeps the selected row visible and stable.
        for h in [1usize, 2] {
            let sc = ensure_visible(0, &ranges, 0, 9, h);
            assert!(
                sc <= line && line < sc + h,
                "h={h} must keep row {line} visible (scroll {sc})"
            );
            assert_eq!(
                ensure_visible(sc, &ranges, 0, 9, h),
                sc,
                "h={h} scroll must be a fixed point"
            );
        }
    }

    // --- column_at_x -----------------------------------------------------

    #[test]
    fn column_at_x_maps_points_to_columns_and_sidebar() {
        let rects = column_rects(area(100, 40), 3);
        // Anywhere in the sidebar → column 0.
        assert_eq!(column_at_x(&rects, 0), Some(0));
        assert_eq!(column_at_x(&rects, rects.sidebar.width - 1), Some(0));

        // Left edge of each column maps to that column (1-based).
        for (i, col) in rects.columns.iter().enumerate() {
            assert_eq!(column_at_x(&rects, col.x), Some(i + 1));
            assert_eq!(column_at_x(&rects, col.x + col.width - 1), Some(i + 1));
        }

        // Separator columns belong to no column.
        assert_eq!(column_at_x(&rects, rects.separators[0]), None);
        // Well past the right edge → nothing.
        assert_eq!(column_at_x(&rects, 200), None);
    }
}
