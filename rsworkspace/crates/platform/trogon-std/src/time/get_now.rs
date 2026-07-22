/// Uses an associated type so each implementation can define its own
/// instant representation (`std::time::Instant` for production,
/// a `Duration` offset for testing).
pub trait GetNow {
    type Instant: Copy;

    fn now(&self) -> Self::Instant;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::GetNow;
    use crate::time::{MockClock, MockInstant};

    #[test]
    fn starts_at_time_zero() {
        let clock = MockClock::new();
        assert_eq!(clock.now(), MockInstant(Duration::ZERO));
    }

    #[test]
    fn now_reflects_advanced_time() {
        let clock = MockClock::new();
        clock.advance(Duration::from_secs(10));

        assert_eq!(clock.now(), MockInstant(Duration::from_secs(10)));
    }

    #[test]
    fn successive_nows_are_equal_without_advance() {
        let clock = MockClock::new();
        clock.advance(Duration::from_millis(42));

        let t1 = clock.now();
        let t2 = clock.now();
        assert_eq!(t1, t2);
    }

    #[test]
    fn now_after_set_reflects_absolute_time() {
        let clock = MockClock::new();
        clock.set(Duration::from_secs(100));

        assert_eq!(clock.now(), MockInstant(Duration::from_secs(100)));
    }

    #[test]
    fn generic_fn_uses_trait_bound() {
        fn snapshot<C: GetNow>(clock: &C) -> C::Instant {
            clock.now()
        }

        let clock = MockClock::new();
        clock.advance(Duration::from_millis(7));
        let instant = snapshot(&clock);

        assert_eq!(instant, MockInstant(Duration::from_millis(7)));
    }
}
