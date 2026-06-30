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
