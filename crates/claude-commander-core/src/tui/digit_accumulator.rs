//! Two-digit number accumulator with debounce for session number jumping.
//!
//! Accepts digit presses one at a time.  A single digit is held for a
//! configurable debounce window; if a second digit arrives in time the two
//! are combined (e.g. 1 then 2 → 12).  Leading zero is rejected.

use std::time::{Duration, Instant};

/// Accumulates one- or two-digit session numbers from individual key presses.
#[derive(Debug)]
pub struct DigitAccumulator {
    /// First digit and when it was pressed, if waiting for a potential second digit.
    pending: Option<(u8, Instant)>,
    /// How long to wait for a second digit before committing the first.
    debounce: Duration,
}

/// Result of feeding a digit or checking the timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigitResult {
    /// No action yet — digit stored, waiting for possible second digit.
    Pending,
    /// A session number is ready to jump to.
    Jump(usize),
    /// The input was ignored (e.g. leading zero).
    Ignored,
}

impl DigitAccumulator {
    /// Create a new accumulator with the given debounce duration.
    pub fn new(debounce: Duration) -> Self {
        Self {
            pending: None,
            debounce,
        }
    }

    /// Feed a digit (0–9) into the accumulator.
    pub fn press(&mut self, digit: u8) -> DigitResult {
        debug_assert!(digit <= 9);

        if let Some((first, _)) = self.pending.take() {
            // Second digit — combine and jump immediately
            DigitResult::Jump(first as usize * 10 + digit as usize)
        } else if digit > 0 {
            // First digit (reject leading zero)
            self.pending = Some((digit, Instant::now()));
            DigitResult::Pending
        } else {
            DigitResult::Ignored
        }
    }

    /// Check whether the debounce window has expired.
    /// Call this on each tick.  Returns `Jump` if the pending digit should
    /// fire, or `None` if there is nothing to do.
    pub fn tick(&mut self) -> Option<DigitResult> {
        let (digit, pressed_at) = self.pending?;
        if pressed_at.elapsed() >= self.debounce {
            self.pending = None;
            Some(DigitResult::Jump(digit as usize))
        } else {
            None
        }
    }

    /// Whether a digit is currently pending (waiting for a second press or timeout).
    #[cfg(test)]
    fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Cancel any pending digit without firing.
    #[cfg(test)]
    fn cancel(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc() -> DigitAccumulator {
        DigitAccumulator::new(Duration::from_millis(250))
    }

    // ── Single digit ──────────────────────────────────────────────

    #[test]
    fn single_digit_returns_pending() {
        let mut a = acc();
        assert_eq!(a.press(3), DigitResult::Pending);
        assert!(a.is_pending());
    }

    #[test]
    fn single_digit_fires_after_debounce() {
        let mut a = DigitAccumulator::new(Duration::ZERO);
        a.press(5);
        // With zero debounce, tick should resolve immediately
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(a.tick(), Some(DigitResult::Jump(5)));
        assert!(!a.is_pending());
    }

    #[test]
    fn single_digit_does_not_fire_before_debounce() {
        let mut a = DigitAccumulator::new(Duration::from_secs(10));
        a.press(7);
        assert_eq!(a.tick(), None);
        assert!(a.is_pending());
    }

    // ── Two digits ────────────────────────────────────────────────

    #[test]
    fn two_digits_combine_immediately() {
        let mut a = acc();
        assert_eq!(a.press(1), DigitResult::Pending);
        assert_eq!(a.press(2), DigitResult::Jump(12));
        assert!(!a.is_pending());
    }

    #[test]
    fn two_digits_max_is_99() {
        let mut a = acc();
        a.press(9);
        assert_eq!(a.press(9), DigitResult::Jump(99));
    }

    #[test]
    fn second_digit_zero_is_valid() {
        let mut a = acc();
        a.press(1);
        assert_eq!(a.press(0), DigitResult::Jump(10));
    }

    #[test]
    fn second_digit_gives_20() {
        let mut a = acc();
        a.press(2);
        assert_eq!(a.press(0), DigitResult::Jump(20));
    }

    // ── Leading zero ──────────────────────────────────────────────

    #[test]
    fn leading_zero_is_ignored() {
        let mut a = acc();
        assert_eq!(a.press(0), DigitResult::Ignored);
        assert!(!a.is_pending());
    }

    #[test]
    fn zero_after_ignored_zero_still_ignored() {
        let mut a = acc();
        a.press(0);
        assert_eq!(a.press(0), DigitResult::Ignored);
    }

    #[test]
    fn digit_after_ignored_zero_starts_fresh() {
        let mut a = acc();
        a.press(0);
        assert_eq!(a.press(3), DigitResult::Pending);
        assert!(a.is_pending());
    }

    // ── Tick with no pending ──────────────────────────────────────

    #[test]
    fn tick_with_no_pending_returns_none() {
        let mut a = acc();
        assert_eq!(a.tick(), None);
    }

    #[test]
    fn tick_after_two_digit_jump_returns_none() {
        let mut a = acc();
        a.press(1);
        a.press(5);
        assert_eq!(a.tick(), None);
    }

    // ── Cancel ────────────────────────────────────────────────────

    #[test]
    fn cancel_clears_pending() {
        let mut a = acc();
        a.press(4);
        a.cancel();
        assert!(!a.is_pending());
        assert_eq!(a.tick(), None);
    }

    // ── Sequences ─────────────────────────────────────────────────

    #[test]
    fn can_jump_again_after_two_digit_jump() {
        let mut a = acc();
        a.press(1);
        assert_eq!(a.press(2), DigitResult::Jump(12));
        // Start a new sequence
        assert_eq!(a.press(5), DigitResult::Pending);
        assert_eq!(a.press(3), DigitResult::Jump(53));
    }

    #[test]
    fn can_jump_again_after_single_digit_timeout() {
        let mut a = DigitAccumulator::new(Duration::ZERO);
        a.press(3);
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(a.tick(), Some(DigitResult::Jump(3)));
        // Start a new sequence
        assert_eq!(a.press(7), DigitResult::Pending);
    }

    // ── All digits 1-9 as first digit ─────────────────────────────

    #[test]
    fn all_single_digits_are_pending() {
        for d in 1..=9u8 {
            let mut a = acc();
            assert_eq!(a.press(d), DigitResult::Pending, "digit {d}");
        }
    }
}
