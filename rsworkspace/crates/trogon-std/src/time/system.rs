use std::time::Duration;

use super::{GetElapsed, GetNow};

/// Zero-sized type — delegates to `std::time::Instant`.
pub struct SystemClock;

impl GetNow for SystemClock {
    type Instant = std::time::Instant;

    #[inline]
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

impl GetElapsed for SystemClock {
    #[inline]
    fn elapsed(&self, since: std::time::Instant) -> Duration {
        since.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_system_clock_now_returns_value() {
        let clock = SystemClock;
        let instant = clock.now();
        let elapsed = clock.elapsed(instant);
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn test_generic_function_with_system_clock() {
        fn is_expired<C: GetNow + GetElapsed>(
            clock: &C,
            started_at: C::Instant,
            ttl: Duration,
        ) -> bool {
            clock.elapsed(started_at) >= ttl
        }

        let clock = SystemClock;
        let start = clock.now();
        assert!(!is_expired(&clock, start, Duration::from_secs(9999)));
    }
}
