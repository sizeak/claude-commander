//! Tree list navigation state.

use ratatui::widgets::ListState;

/// Tree list state
#[derive(Debug, Default)]
pub struct TreeListState {
    /// Inner list state
    pub list_state: ListState,
    /// Total number of items
    pub item_count: usize,
    /// Per-index selectability (empty = all selectable).
    selectable: Vec<bool>,
    /// Per-index group-start flags — rows that begin a group, i.e. project
    /// or section headers (empty = no groups).
    group_starts: Vec<bool>,
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

    fn is_selectable(&self, idx: usize) -> bool {
        self.selectable.get(idx).copied().unwrap_or(true)
    }

    fn is_group_start(&self, idx: usize) -> bool {
        self.group_starts.get(idx).copied().unwrap_or(false)
    }

    fn any_selectable(&self) -> bool {
        if self.selectable.is_empty() {
            return self.item_count > 0;
        }
        self.selectable.iter().any(|s| *s)
    }

    /// Select the next item, skipping unselectable rows.
    pub fn next(&mut self) {
        if !self.any_selectable() {
            return;
        }
        let count = self.item_count;
        let start = self
            .list_state
            .selected()
            .map(|i| (i + 1) % count)
            .unwrap_or(0);
        for offset in 0..count {
            let i = (start + offset) % count;
            if self.is_selectable(i) {
                self.list_state.select(Some(i));
                return;
            }
        }
    }

    /// Select the previous item, skipping unselectable rows.
    pub fn previous(&mut self) {
        if !self.any_selectable() {
            return;
        }
        let count = self.item_count;
        let start = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    count - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        for offset in 0..count {
            let i = (start + count - offset) % count;
            if self.is_selectable(i) {
                self.list_state.select(Some(i));
                return;
            }
        }
    }

    /// Move the selection by `rows` rows in one jump, clamping at the ends of
    /// the list instead of wrapping like [`next`](Self::next) /
    /// [`previous`](Self::previous) — a page jump that teleported to the far
    /// end of the list would be disorienting (the same reasoning that keeps
    /// `list_nav::wheel_step` clamping). A `rows` of 0 moves one row.
    ///
    /// Lands on the nearest selectable row at or past the target in the
    /// direction of travel, falling back to the closest one behind it, so
    /// header and divider rows are skipped.
    pub fn page(&mut self, rows: usize, down: bool) {
        if !self.any_selectable() {
            return;
        }
        let rows = rows.max(1);
        let current = self.list_state.selected().unwrap_or(0);
        let target = if down {
            (current + rows).min(self.item_count - 1)
        } else {
            current.saturating_sub(rows)
        };
        self.select_nearest_toward(target, down);
    }

    /// Select the next group header (project/section row), wrapping past
    /// the end. No-op when no selectable group start exists.
    pub fn next_group(&mut self) {
        let count = self.item_count;
        if count == 0 {
            return;
        }
        let start = self
            .list_state
            .selected()
            .map(|i| (i + 1) % count)
            .unwrap_or(0);
        for offset in 0..count {
            let i = (start + offset) % count;
            if self.is_group_start(i) && self.is_selectable(i) {
                self.list_state.select(Some(i));
                return;
            }
        }
    }

    /// Select the previous group header, wrapping past the start. From a
    /// row inside a group this lands on that group's own header; from a
    /// header, on the previous group's header. No-op when no selectable
    /// group start exists.
    pub fn previous_group(&mut self) {
        let count = self.item_count;
        if count == 0 {
            return;
        }
        let start = match self.list_state.selected() {
            Some(0) | None => count - 1,
            Some(i) => i - 1,
        };
        for offset in 0..count {
            let i = (start + count - offset) % count;
            if self.is_group_start(i) && self.is_selectable(i) {
                self.list_state.select(Some(i));
                return;
            }
        }
    }

    /// Select the first selectable item. No-op on an empty list.
    pub fn select_first(&mut self) {
        if let Some(i) = (0..self.item_count).find(|&i| self.is_selectable(i)) {
            self.list_state.select(Some(i));
        }
    }

    /// Select the last selectable item. No-op on an empty list.
    pub fn select_last(&mut self) {
        if let Some(i) = (0..self.item_count).rev().find(|&i| self.is_selectable(i)) {
            self.list_state.select(Some(i));
        }
    }

    /// Select the nearest selectable row to `idx`, preferring the row at or
    /// after `idx` and falling back to the closest selectable row before it.
    ///
    /// Used after a deletion rebuilds the list: passing the removed row's old
    /// index lands the cursor on the row that slid up into its place (the next
    /// sibling) — or on the previous row when the last row was removed —
    /// rather than resetting to the top. No-op on a list with no selectable
    /// rows (selection is cleared).
    pub fn select_nearest(&mut self, idx: usize) {
        if !self.any_selectable() {
            self.list_state.select(None);
            return;
        }
        self.select_nearest_toward(idx, true);
    }

    /// Select the nearest selectable row to `idx`, searching in the direction
    /// given by `forward` first and falling back to the opposite one.
    fn select_nearest_toward(&mut self, idx: usize, forward: bool) {
        let count = self.item_count;
        if count == 0 {
            return;
        }
        let found = if forward {
            (idx..count)
                .find(|&i| self.is_selectable(i))
                .or_else(|| (0..count.min(idx)).rev().find(|&i| self.is_selectable(i)))
        } else {
            (0..=idx.min(count - 1))
                .rev()
                .find(|&i| self.is_selectable(i))
                .or_else(|| (idx + 1..count).find(|&i| self.is_selectable(i)))
        };
        if let Some(i) = found {
            self.list_state.select(Some(i));
        }
    }

    /// Update item count and ensure selection is valid.
    ///
    /// Also clears any per-index `selectable` and `group_starts` masks
    /// installed by prior `set_selectable`/`set_group_starts` calls —
    /// `set_item_count` is the "no mask, every row is selectable" entry
    /// point, and a stale mask from another view would otherwise make rows
    /// at the same indices unreachable with up/down navigation (or send
    /// group jumps to rows that are no longer headers).
    pub fn set_item_count(&mut self, count: usize) {
        self.item_count = count;
        self.selectable.clear();
        self.group_starts.clear();

        // Ensure selection is still valid
        if let Some(selected) = self.list_state.selected() {
            if selected >= count && count > 0 {
                self.list_state.select(Some(count - 1));
            } else if count == 0 {
                self.list_state.select(None);
            }
        }
    }

    /// Set a per-index selectable mask. The mask length should equal the
    /// current item count; shorter masks default unknown indices to selectable.
    /// Also updates item count to match mask length.
    pub fn set_selectable(&mut self, mask: Vec<bool>) {
        self.item_count = mask.len();
        self.selectable = mask;
        if let Some(sel) = self.list_state.selected()
            && (sel >= self.item_count || !self.is_selectable(sel))
        {
            self.list_state.select(None);
        }
    }

    /// Set the per-index group-start mask (which rows are project/section
    /// headers). Call after `set_item_count`/`set_selectable`, which reset
    /// or resize the list. Shorter masks default unknown indices to "not a
    /// group".
    pub fn set_group_starts(&mut self, mask: Vec<bool>) {
        self.group_starts = mask;
    }
}
