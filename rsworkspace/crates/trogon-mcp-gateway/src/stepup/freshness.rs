//! Injectable clock for `auth_time` freshness checks.

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock source for step-up freshness (swap in tests).
pub trait FreshnessClock {
    fn now_unix(&self) -> i64;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SystemFreshnessClock;

impl FreshnessClock for SystemFreshnessClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// Returns true when `auth_time` is within `max_age_seconds` of `clock.now_unix()`.
#[must_use]
pub fn is_auth_time_fresh(
    clock: &impl FreshnessClock,
    auth_time: i64,
    max_age_seconds: u64,
) -> bool {
    let age = clock.now_unix().saturating_sub(auth_time);
    age <= i64::try_from(max_age_seconds).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock(i64);

    impl FreshnessClock for FixedClock {
        fn now_unix(&self) -> i64 {
            self.0
        }
    }

    #[test]
    fn fresh_when_age_equals_max() {
        let clock = FixedClock(1_000);
        assert!(is_auth_time_fresh(&clock, 700, 300));
    }

    #[test]
    fn stale_when_age_exceeds_max() {
        let clock = FixedClock(1_000);
        assert!(!is_auth_time_fresh(&clock, 699, 300));
    }
}
