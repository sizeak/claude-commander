//! Tree list navigation state.

use ratatui::widgets::ListState;

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
