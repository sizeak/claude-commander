//! Shared primitives for mouse interaction with row lists.
//!
//! Used by both the in-app list modals (`tui::app`) and the standalone
//! in-session picker (`crate::picker`), which runs in a tmux popup outside
//! the app event loop but must offer the same interactions: one click
//! highlights a row, a second click on the same row within
//! [`DOUBLE_CLICK_WINDOW`] activates it, and the wheel moves the highlight.

use std::time::Duration;

use ratatui::layout::Rect;

/// Maximum delay between two same-row left clicks for them to count as a
/// double-click that activates the row.
pub(crate) const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// One mouse-wheel step over a list selection: move a single row, clamping
/// at the ends rather than wrapping like keyboard navigation — a wheel tick
/// at the bottom of a list jumping back to the top would be disorienting.
pub(crate) fn wheel_step(selected_idx: usize, down: bool, len: usize) -> usize {
    if down {
        (selected_idx + 1).min(len.saturating_sub(1))
    } else {
        selected_idx.saturating_sub(1)
    }
}

/// Map a mouse position to an absolute list index. `rows` is the rows-only
/// area recorded at render time, `scroll` the index of the first visible
/// row, `len` the list length. Returns `None` for positions outside `rows`
/// or on an unpopulated row below the end of the list.
pub(crate) fn list_index_at(
    col: u16,
    row: u16,
    rows: Rect,
    scroll: usize,
    len: usize,
) -> Option<usize> {
    let inside =
        col >= rows.x && col < rows.x + rows.width && row >= rows.y && row < rows.y + rows.height;
    if !inside {
        return None;
    }
    let idx = scroll + (row - rows.y) as usize;
    (idx < len).then_some(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_index_at_maps_rows_within_bounds() {
        let rows = Rect::new(20, 12, 58, 5);
        assert_eq!(list_index_at(20, 12, rows, 0, 10), Some(0));
        assert_eq!(list_index_at(77, 16, rows, 0, 10), Some(4));
        // Left/right of the rows area, the input line above, below the window.
        assert_eq!(list_index_at(19, 12, rows, 0, 10), None);
        assert_eq!(list_index_at(78, 12, rows, 0, 10), None);
        assert_eq!(list_index_at(20, 11, rows, 0, 10), None);
        assert_eq!(list_index_at(20, 17, rows, 0, 10), None);
    }

    #[test]
    fn list_index_at_applies_scroll_offset() {
        let rows = Rect::new(20, 12, 58, 5);
        assert_eq!(list_index_at(20, 13, rows, 3, 10), Some(4));
    }

    #[test]
    fn list_index_at_rejects_rows_past_end_of_list() {
        let rows = Rect::new(20, 12, 58, 5);
        assert_eq!(list_index_at(20, 14, rows, 0, 2), None);
        assert_eq!(list_index_at(20, 12, rows, 0, 0), None);
    }

    #[test]
    fn wheel_step_moves_one_row_and_clamps_at_ends() {
        assert_eq!(wheel_step(0, false, 5), 0);
        assert_eq!(wheel_step(2, false, 5), 1);
        assert_eq!(wheel_step(2, true, 5), 3);
        assert_eq!(wheel_step(4, true, 5), 4);
    }
}
