//! Reconnect backoff schedule — a pure function of the failure count so it can
//! be unit-tested deterministically (no clock, no sleeping).

use std::time::Duration;

/// Exponential-backoff parameters for the poller's reconnect loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackoffConfig {
    /// Delay after the first failure.
    pub initial: Duration,
    /// Ceiling the delay is clamped to.
    pub max: Duration,
    /// Multiplier applied per additional consecutive failure.
    pub factor: u32,
}

impl Default for BackoffConfig {
    /// 1s doubling, capped at 60s — the schedule Phase E specifies.
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            factor: 2,
        }
    }
}

/// Delay to wait before the retry that follows `consecutive_failures` failures
/// in a row (1-based: `1` is the delay after the first failure). Returns
/// [`Duration::ZERO`] for `0` (no failure yet). The delay is
/// `initial * factor^(consecutive_failures - 1)`, saturating on overflow and
/// clamped to `max`, so an arbitrarily long outage settles at a steady `max`.
pub fn backoff_delay(config: &BackoffConfig, consecutive_failures: u32) -> Duration {
    if consecutive_failures == 0 {
        return Duration::ZERO;
    }
    let exponent = consecutive_failures - 1;
    let multiplier = (config.factor as u64)
        .checked_pow(exponent)
        .unwrap_or(u64::MAX);
    let millis = (config.initial.as_millis() as u64).saturating_mul(multiplier);
    Duration::from_millis(millis).min(config.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BackoffConfig {
        BackoffConfig::default()
    }

    #[test]
    fn zero_failures_is_no_delay() {
        assert_eq!(backoff_delay(&cfg(), 0), Duration::ZERO);
    }

    #[test]
    fn doubles_from_the_initial_delay() {
        assert_eq!(backoff_delay(&cfg(), 1), Duration::from_secs(1));
        assert_eq!(backoff_delay(&cfg(), 2), Duration::from_secs(2));
        assert_eq!(backoff_delay(&cfg(), 3), Duration::from_secs(4));
        assert_eq!(backoff_delay(&cfg(), 4), Duration::from_secs(8));
        assert_eq!(backoff_delay(&cfg(), 5), Duration::from_secs(16));
        assert_eq!(backoff_delay(&cfg(), 6), Duration::from_secs(32));
    }

    #[test]
    fn clamps_to_max() {
        // 2^6 = 64s would exceed the 60s cap, and everything beyond stays there.
        assert_eq!(backoff_delay(&cfg(), 7), Duration::from_secs(60));
        assert_eq!(backoff_delay(&cfg(), 8), Duration::from_secs(60));
        assert_eq!(backoff_delay(&cfg(), 100), Duration::from_secs(60));
    }

    #[test]
    fn huge_failure_count_saturates_without_panicking() {
        // A pow overflow must saturate to `max`, not panic or wrap to a tiny value.
        assert_eq!(backoff_delay(&cfg(), u32::MAX), Duration::from_secs(60));
    }

    #[test]
    fn respects_custom_parameters() {
        let custom = BackoffConfig {
            initial: Duration::from_millis(100),
            max: Duration::from_secs(1),
            factor: 3,
        };
        assert_eq!(backoff_delay(&custom, 1), Duration::from_millis(100));
        assert_eq!(backoff_delay(&custom, 2), Duration::from_millis(300));
        assert_eq!(backoff_delay(&custom, 3), Duration::from_millis(900));
        // 100ms * 3^3 = 2700ms, clamped to the 1s max.
        assert_eq!(backoff_delay(&custom, 4), Duration::from_secs(1));
    }
}
