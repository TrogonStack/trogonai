use std::time::Duration;

use super::GetNow;

pub trait GetElapsed: GetNow {
    fn elapsed(&self, since: Self::Instant) -> Duration;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::GetElapsed;
    use crate::time::{GetNow, MockClock};

    #[test]
    fn elapsed_is_zero_at_start() {
        let clock = MockClock::new();
        let t0 = clock.now();
        assert_eq!(clock.elapsed(t0), Duration::ZERO);
    }

    #[test]
    fn elapsed_matches_advance() {
        let clock = MockClock::new();
        let t0 = clock.now();
        clock.advance(Duration::from_secs(5));

        assert_eq!(clock.elapsed(t0), Duration::from_secs(5));
    }

    #[test]
    fn elapsed_accumulates_across_multiple_advances() {
        let clock = MockClock::new();
        let t0 = clock.now();
        clock.advance(Duration::from_secs(3));
        clock.advance(Duration::from_secs(2));

        assert_eq!(clock.elapsed(t0), Duration::from_secs(5));
    }

    #[test]
    fn elapsed_from_later_snapshot_is_less() {
        let clock = MockClock::new();
        clock.advance(Duration::from_secs(10));
        let t_mid = clock.now();
        clock.advance(Duration::from_secs(5));

        assert_eq!(clock.elapsed(t_mid), Duration::from_secs(5));
    }

    #[test]
    fn elapsed_saturates_to_zero_when_clock_goes_back() {
        let clock = MockClock::new();
        clock.set(Duration::from_secs(20));
        let future = clock.now();
        clock.set(Duration::from_secs(10));

        assert_eq!(clock.elapsed(future), Duration::ZERO);
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn has_expired<C: GetNow + GetElapsed>(
            clock: &C,
            since: C::Instant,
            ttl: Duration,
        ) -> bool {
            clock.elapsed(since) >= ttl
        }

        let clock = MockClock::new();
        let start = clock.now();
        let ttl = Duration::from_secs(10);

        assert!(!has_expired(&clock, start, ttl));
        clock.advance(Duration::from_secs(9));
        assert!(!has_expired(&clock, start, ttl));
        clock.advance(Duration::from_secs(1));
        assert!(has_expired(&clock, start, ttl));
    }
}
