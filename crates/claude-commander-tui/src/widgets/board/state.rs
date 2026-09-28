//! Kanban board navigation state.
//!
//! [`BoardState`] tracks the cursor position on the board plus per-column
//! scroll offsets. It is the board analogue of the old tree list's navigation
//! state: the same wrap-around next/previous and `sync`/`select_nearest`
//! semantics, generalised to a two-dimensional grid of columns.
//!
//! The board is addressed by [`BoardPos`], where `col == 0` is the project
//! sidebar and `col` in `1..=N` are the section columns. `counts[0]` is the
//! sidebar's project count and `counts[1..]` are the per-column selectable row
//! counts (the flattened Worktree-row count of each section column).

use claude_commander_core::session::BoardPos;

/// Navigation state for the kanban board.
///
/// Holds the current selection, the per-addressable-column scroll offsets
/// (`scroll[col]` in display lines, indexed the same way as `counts`), and the
/// selectable-row counts captured at the last [`sync`](BoardState::sync).
#[derive(Debug, Default, Clone)]
pub struct BoardState {
    /// Current cursor position, or `None` when the board is empty.
    selected: Option<BoardPos>,
    /// Per-column vertical scroll offset in display lines. Same length and
    /// indexing as `counts` (index 0 = sidebar). Public so the widget can read
    /// and update it during render via [`layout::ensure_visible`](super::layout::ensure_visible).
    pub scroll: Vec<usize>,
    /// Selectable-row count per addressable column. `counts[0]` is the sidebar
    /// project count; `counts[1..]` are the section columns' flattened row
    /// counts. Captured at the last `sync`.
    counts: Vec<usize>,
}

impl BoardState {
    /// Create an empty state with no selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current selection, or `None` when the board is empty.
    pub fn selected(&self) -> Option<BoardPos> {
        self.selected
    }

    /// The selected column index (0 = sidebar), or `None` when nothing is
    /// selected. A landed-but-empty column still reports its column here.
    pub fn selected_column(&self) -> Option<usize> {
        self.selected.map(|p| p.col)
    }

    /// Set the selection directly. No clamping is applied — callers that need a
    /// valid position should route through [`sync`](Self::sync) or the
    /// navigation methods.
    pub fn select(&mut self, pos: Option<BoardPos>) {
        self.selected = pos;
    }

    /// Reconcile the state after a board rebuild.
    ///
    /// `counts[0]` is the sidebar project count and `counts[1..]` are the
    /// per-section-column selectable row counts. This:
    /// - resizes `scroll` to match, preserving existing offsets where possible
    ///   and zeroing any column that became empty;
    /// - clamps an existing selection: the column to the valid range, and the
    ///   row to that column's count (a selection in a now-shorter column moves
    ///   to its last row; a column that emptied keeps the selection on the
    ///   column at row 0 — the "header position");
    /// - installs a default selection when there was none: the first row of the
    ///   first non-empty section column (preferring `col >= 1` over the
    ///   sidebar), falling back to the sidebar's first row if only projects
    ///   exist, and `None` if the board is entirely empty.
    pub fn sync(&mut self, counts: Vec<usize>) {
        self.counts = counts;

        // Resize scroll, preserving existing offsets. A column that is now
        // empty has nothing to scroll, so reset it.
        self.scroll.resize(self.counts.len(), 0);
        for (col, &count) in self.counts.iter().enumerate() {
            if count == 0 {
                self.scroll[col] = 0;
            }
        }

        match self.selected {
            Some(pos) => self.selected = Some(self.clamp(pos)),
            None => self.selected = self.default_selection(),
        }
    }

    /// Clamp a position to the current `counts`: column into range, then row to
    /// that column's count (0 when the column is empty — the header position).
    fn clamp(&self, pos: BoardPos) -> BoardPos {
        if self.counts.is_empty() {
            return BoardPos { col: 0, row: 0 };
        }
        let col = pos.col.min(self.counts.len() - 1);
        let count = self.counts[col];
        let row = if count == 0 {
            0
        } else {
            pos.row.min(count - 1)
        };
        BoardPos { col, row }
    }

    /// The default selection for a fresh/empty state: first row of the first
    /// non-empty section column, else the sidebar's first row, else `None`.
    fn default_selection(&self) -> Option<BoardPos> {
        for (col, &count) in self.counts.iter().enumerate().skip(1) {
            if count > 0 {
                return Some(BoardPos { col, row: 0 });
            }
        }
        if self.counts.first().copied().unwrap_or(0) > 0 {
            return Some(BoardPos { col: 0, row: 0 });
        }
        None
    }

    /// Move to the next row within the current column, wrapping past the last
    /// row. No-op when there is no selection or the current column is empty.
    pub fn next_row(&mut self) {
        if let Some(pos) = self.selected {
            let count = self.counts.get(pos.col).copied().unwrap_or(0);
            if count == 0 {
                return;
            }
            self.selected = Some(BoardPos {
                col: pos.col,
                row: (pos.row + 1) % count,
            });
        }
    }

    /// Move to the previous row within the current column, wrapping past the
    /// first row. No-op when there is no selection or the current column is
    /// empty.
    pub fn previous_row(&mut self) {
        if let Some(pos) = self.selected {
            let count = self.counts.get(pos.col).copied().unwrap_or(0);
            if count == 0 {
                return;
            }
            self.selected = Some(BoardPos {
                col: pos.col,
                row: (pos.row + count - 1) % count,
            });
        }
    }

    /// Move `rows` rows up or down within the current column, clamping at the
    /// ends rather than wrapping (a page jump that wrapped would be
    /// disorienting). No-op when there is no selection or the column is empty.
    pub fn page(&mut self, rows: usize, down: bool) {
        if let Some(pos) = self.selected {
            let count = self.counts.get(pos.col).copied().unwrap_or(0);
            if count == 0 {
                return;
            }
            let rows = rows.max(1);
            let row = if down {
                (pos.row + rows).min(count - 1)
            } else {
                pos.row.saturating_sub(rows)
            };
            self.selected = Some(BoardPos { col: pos.col, row });
        }
    }

    /// Move to the next column, wrapping past the last (including the sidebar
    /// at column 0). The row is clamped to the target column's count; an empty
    /// target column is still landable (row 0 — the header position). No-op
    /// when there is no selection or no columns.
    pub fn next_column(&mut self) {
        if let Some(pos) = self.selected {
            let ncols = self.counts.len();
            if ncols == 0 {
                return;
            }
            let col = (pos.col + 1) % ncols;
            self.selected = Some(self.land_in_column(col, pos.row));
        }
    }

    /// Move to the previous column, wrapping past the first (including the
    /// sidebar at column 0). See [`next_column`](Self::next_column) for row
    /// handling.
    pub fn previous_column(&mut self) {
        if let Some(pos) = self.selected {
            let ncols = self.counts.len();
            if ncols == 0 {
                return;
            }
            let col = (pos.col + ncols - 1) % ncols;
            self.selected = Some(self.land_in_column(col, pos.row));
        }
    }

    /// Compute the landing position in `col`, keeping `preferred_row` but
    /// clamping it to the column's count (row 0 for an empty column).
    fn land_in_column(&self, col: usize, preferred_row: usize) -> BoardPos {
        let count = self.counts.get(col).copied().unwrap_or(0);
        let row = if count == 0 {
            0
        } else {
            preferred_row.min(count - 1)
        };
        BoardPos { col, row }
    }

    /// Select the first row of the current column. No-op without a selection.
    pub fn select_first(&mut self) {
        if let Some(pos) = self.selected {
            self.selected = Some(BoardPos {
                col: pos.col,
                row: 0,
            });
        }
    }

    /// Select the last row of the current column (row 0 when empty). No-op
    /// without a selection.
    pub fn select_last(&mut self) {
        if let Some(pos) = self.selected {
            let count = self.counts.get(pos.col).copied().unwrap_or(0);
            self.selected = Some(BoardPos {
                col: pos.col,
                row: count.saturating_sub(1),
            });
        }
    }

    /// Select the nearest surviving row after a deletion at `pos`.
    ///
    /// Stays in `pos.col` and picks the row that slid up into the deleted
    /// position (`pos.row`), falling back to the previous row when the last row
    /// was removed. If the column is now empty, the selection stays on the
    /// column at row 0 (the header position). `pos.col` is clamped into range
    /// defensively.
    pub fn select_nearest(&mut self, pos: BoardPos) {
        self.selected = if self.counts.is_empty() {
            None
        } else {
            Some(self.clamp(pos))
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(col: usize, row: usize) -> BoardPos {
        BoardPos { col, row }
    }

    // --- sync: default selection -----------------------------------------

    #[test]
    fn default_selection_prefers_first_non_empty_section_column() {
        let mut state = BoardState::new();
        // sidebar=2, In Progress=0, "Open"=3 → land on the first non-empty
        // section column, not the sidebar.
        state.sync(vec![2, 0, 3]);
        assert_eq!(state.selected(), Some(pos(2, 0)));
    }

    #[test]
    fn default_selection_falls_back_to_sidebar_when_only_projects_exist() {
        let mut state = BoardState::new();
        // Projects exist but every section column is empty.
        state.sync(vec![2, 0, 0]);
        assert_eq!(state.selected(), Some(pos(0, 0)));
    }

    #[test]
    fn default_selection_is_none_on_a_wholly_empty_board() {
        let mut state = BoardState::new();
        state.sync(vec![0, 0, 0]);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn default_selection_uses_first_non_empty_even_with_earlier_empty_column() {
        let mut state = BoardState::new();
        // In Progress empty, second section column populated.
        state.sync(vec![0, 0, 5]);
        assert_eq!(state.selected(), Some(pos(2, 0)));
    }

    // --- sync: clamping an existing selection ----------------------------

    #[test]
    fn sync_preserves_a_still_valid_selection() {
        let mut state = BoardState::new();
        state.sync(vec![1, 3, 3]);
        state.select(Some(pos(1, 2)));
        state.sync(vec![1, 3, 3]);
        assert_eq!(state.selected(), Some(pos(1, 2)));
    }

    #[test]
    fn sync_clamps_row_to_last_when_column_shrinks() {
        let mut state = BoardState::new();
        state.sync(vec![1, 5, 1]);
        state.select(Some(pos(1, 4)));
        // Column 1 shrank from 5 rows to 2.
        state.sync(vec![1, 2, 1]);
        assert_eq!(state.selected(), Some(pos(1, 1)));
    }

    #[test]
    fn sync_keeps_column_at_header_when_it_empties() {
        let mut state = BoardState::new();
        state.sync(vec![1, 3, 1]);
        state.select(Some(pos(1, 2)));
        state.sync(vec![1, 0, 1]);
        assert_eq!(state.selected(), Some(pos(1, 0)));
    }

    #[test]
    fn sync_clamps_column_when_columns_shrink() {
        let mut state = BoardState::new();
        state.sync(vec![1, 2, 2, 2]);
        state.select(Some(pos(3, 1)));
        // Two section columns removed; only sidebar + one column remain.
        state.sync(vec![1, 2]);
        assert_eq!(state.selected(), Some(pos(1, 1)));
    }

    #[test]
    fn sync_resizes_scroll_preserving_offsets() {
        let mut state = BoardState::new();
        state.sync(vec![1, 3, 3]);
        state.scroll[1] = 4;
        state.sync(vec![1, 3, 3, 3]);
        assert_eq!(state.scroll.len(), 4);
        assert_eq!(state.scroll[1], 4, "existing offset preserved");
        assert_eq!(state.scroll[3], 0, "new column starts unscrolled");
    }

    #[test]
    fn sync_zeroes_scroll_for_emptied_column() {
        let mut state = BoardState::new();
        state.sync(vec![1, 3, 3]);
        state.scroll[1] = 5;
        state.sync(vec![1, 0, 3]);
        assert_eq!(state.scroll[1], 0);
    }

    // --- next_row / previous_row -----------------------------------------

    #[test]
    fn next_row_wraps_within_column() {
        let mut state = BoardState::new();
        state.sync(vec![1, 3, 1]);
        state.select(Some(pos(1, 0)));
        state.next_row();
        assert_eq!(state.selected(), Some(pos(1, 1)));
        state.next_row();
        assert_eq!(state.selected(), Some(pos(1, 2)));
        state.next_row();
        assert_eq!(state.selected(), Some(pos(1, 0)), "wraps past the last row");
    }

    #[test]
    fn previous_row_wraps_within_column() {
        let mut state = BoardState::new();
        state.sync(vec![1, 3, 1]);
        state.select(Some(pos(1, 0)));
        state.previous_row();
        assert_eq!(
            state.selected(),
            Some(pos(1, 2)),
            "wraps past the first row"
        );
        state.previous_row();
        assert_eq!(state.selected(), Some(pos(1, 1)));
    }

    #[test]
    fn row_nav_is_a_noop_in_an_empty_column() {
        let mut state = BoardState::new();
        state.sync(vec![1, 0, 1]);
        state.select(Some(pos(1, 0)));
        state.next_row();
        assert_eq!(state.selected(), Some(pos(1, 0)));
        state.previous_row();
        assert_eq!(state.selected(), Some(pos(1, 0)));
    }

    // --- next_column / previous_column -----------------------------------

    #[test]
    fn next_column_wraps_across_all_columns_including_sidebar() {
        let mut state = BoardState::new();
        state.sync(vec![2, 2, 2]);
        state.select(Some(pos(0, 0)));
        state.next_column();
        assert_eq!(state.selected(), Some(pos(1, 0)));
        state.next_column();
        assert_eq!(state.selected(), Some(pos(2, 0)));
        state.next_column();
        assert_eq!(
            state.selected(),
            Some(pos(0, 0)),
            "wraps back to the sidebar"
        );
    }

    #[test]
    fn previous_column_wraps_across_all_columns_including_sidebar() {
        let mut state = BoardState::new();
        state.sync(vec![2, 2, 2]);
        state.select(Some(pos(0, 0)));
        state.previous_column();
        assert_eq!(
            state.selected(),
            Some(pos(2, 0)),
            "wraps to the last column"
        );
    }

    #[test]
    fn column_switch_clamps_row_to_target_count() {
        let mut state = BoardState::new();
        state.sync(vec![1, 5, 2]);
        state.select(Some(pos(1, 4)));
        state.next_column();
        assert_eq!(
            state.selected(),
            Some(pos(2, 1)),
            "row clamped to target's last"
        );
    }

    #[test]
    fn empty_column_is_landable() {
        let mut state = BoardState::new();
        state.sync(vec![1, 3, 0]);
        state.select(Some(pos(1, 2)));
        state.next_column();
        assert_eq!(
            state.selected(),
            Some(pos(2, 0)),
            "an empty column lands at row 0 (header position)"
        );
        assert_eq!(state.selected_column(), Some(2));
    }

    // --- select_first / select_last --------------------------------------

    #[test]
    fn select_first_and_last_within_column() {
        let mut state = BoardState::new();
        state.sync(vec![1, 4, 1]);
        state.select(Some(pos(1, 2)));
        state.select_first();
        assert_eq!(state.selected(), Some(pos(1, 0)));
        state.select_last();
        assert_eq!(state.selected(), Some(pos(1, 3)));
    }

    #[test]
    fn select_last_on_empty_column_stays_at_header() {
        let mut state = BoardState::new();
        state.sync(vec![1, 0, 1]);
        state.select(Some(pos(1, 0)));
        state.select_last();
        assert_eq!(state.selected(), Some(pos(1, 0)));
    }

    // --- select_nearest --------------------------------------------------

    #[test]
    fn select_nearest_after_last_row_delete_picks_previous() {
        let mut state = BoardState::new();
        // Column had 3 rows; the last (row 2) was deleted, so count is now 2.
        state.sync(vec![1, 2, 1]);
        state.select_nearest(pos(1, 2));
        assert_eq!(state.selected(), Some(pos(1, 1)));
    }

    #[test]
    fn select_nearest_keeps_slid_up_row_when_middle_deleted() {
        let mut state = BoardState::new();
        // Column had 3 rows; row 1 deleted, count now 2 — the row that slid up
        // into index 1 is selected.
        state.sync(vec![1, 2, 1]);
        state.select_nearest(pos(1, 1));
        assert_eq!(state.selected(), Some(pos(1, 1)));
    }

    #[test]
    fn select_nearest_on_emptied_column_keeps_column() {
        let mut state = BoardState::new();
        state.sync(vec![1, 0, 1]);
        state.select_nearest(pos(1, 0));
        assert_eq!(state.selected(), Some(pos(1, 0)));
        assert_eq!(state.selected_column(), Some(1));
    }

    #[test]
    fn selected_column_is_some_for_a_landed_empty_column() {
        let mut state = BoardState::new();
        state.sync(vec![1, 0, 0]);
        // Default selection falls back to the sidebar here; force onto an
        // empty column to confirm the column is still reported.
        state.select(Some(pos(2, 0)));
        assert_eq!(state.selected_column(), Some(2));
    }

    #[test]
    fn page_moves_a_screenful_within_the_column() {
        let mut state = BoardState::new();
        state.sync(vec![1, 20]);
        state.select(Some(pos(1, 0)));
        state.page(5, true);
        assert_eq!(state.selected(), Some(pos(1, 5)));
        state.page(5, false);
        assert_eq!(state.selected(), Some(pos(1, 0)));
    }

    #[test]
    fn page_clamps_at_the_ends_instead_of_wrapping() {
        // Unlike `next_row`/`previous_row`, a page jump must not wrap — landing
        // at the far end of the column after one keypress is disorienting.
        let mut state = BoardState::new();
        state.sync(vec![1, 3]);
        state.select(Some(pos(1, 0)));
        state.page(10, true);
        assert_eq!(state.selected(), Some(pos(1, 2)));
        state.page(10, true);
        assert_eq!(
            state.selected(),
            Some(pos(1, 2)),
            "no wrap past the last row"
        );
        state.page(10, false);
        assert_eq!(state.selected(), Some(pos(1, 0)));
        state.page(10, false);
        assert_eq!(
            state.selected(),
            Some(pos(1, 0)),
            "no wrap past the first row"
        );
    }

    #[test]
    fn page_is_a_noop_on_an_empty_column_and_degrades_to_one_row() {
        let mut state = BoardState::new();
        state.sync(vec![1, 0]);
        state.select(Some(pos(1, 0)));
        state.page(5, true);
        assert_eq!(state.selected(), Some(pos(1, 0)));

        // A zero page size (no render height recorded yet) still moves a row.
        state.sync(vec![1, 4]);
        state.select(Some(pos(1, 0)));
        state.page(0, true);
        assert_eq!(state.selected(), Some(pos(1, 1)));
    }
}
