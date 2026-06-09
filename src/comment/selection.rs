//! Pure line-range selection math for the diff view's vim-style visual mode.
//!
//! A selection is the pair `(anchor, active)` of body-line indices. The anchor
//! stays put; the active end moves with the arrow keys, growing or shrinking
//! the range depending on which side of the anchor it lands. Kept here (rather
//! than in the TUI) so it is unit-testable without a terminal.

/// Direction the active end of a selection moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionDir {
    Up,
    Down,
}

/// Normalise a `(anchor, active)` selection into an inclusive `(lo, hi)` range,
/// regardless of drag direction.
pub fn selected_range(visual: (usize, usize)) -> (usize, usize) {
    let (a, b) = visual;
    (a.min(b), a.max(b))
}

/// Move the active end of a selection one line in `dir`, clamped to
/// `[0, max]` (`max` is the last selectable body-line index). The anchor is
/// unchanged, so moving back toward the anchor shrinks the range and moving
/// away grows it.
pub fn grow_or_shrink(visual: (usize, usize), dir: SelectionDir, max: usize) -> (usize, usize) {
    let (anchor, active) = visual;
    let active = match dir {
        SelectionDir::Up => active.saturating_sub(1),
        SelectionDir::Down => (active + 1).min(max),
    };
    (anchor, active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_range_normalises_regardless_of_direction() {
        assert_eq!(selected_range((5, 8)), (5, 8));
        assert_eq!(selected_range((8, 5)), (5, 8));
        assert_eq!(selected_range((3, 3)), (3, 3));
    }

    #[test]
    fn grow_then_shrink_returns_to_anchor() {
        let v = (5, 5);
        let v = grow_or_shrink(v, SelectionDir::Down, 10); // (5,6) -> range 5..=6
        assert_eq!(selected_range(v), (5, 6));
        let v = grow_or_shrink(v, SelectionDir::Up, 10); // (5,5) -> back to anchor
        assert_eq!(selected_range(v), (5, 5));
    }

    #[test]
    fn grows_upward_past_anchor() {
        let v = (5, 5);
        let v = grow_or_shrink(v, SelectionDir::Up, 10); // (5,4) -> range 4..=5
        assert_eq!(selected_range(v), (4, 5));
    }

    #[test]
    fn clamps_at_bounds() {
        assert_eq!(grow_or_shrink((0, 0), SelectionDir::Up, 10), (0, 0));
        assert_eq!(grow_or_shrink((9, 10), SelectionDir::Down, 10), (9, 10));
    }
}
