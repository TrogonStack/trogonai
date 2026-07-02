use std::time::Duration;

use trogon_std::time::{GetElapsed, GetNow};

use crate::FeedVerdict;

/// A cached feed verdict tagged with the clock instant it was produced.
/// Freshness is evaluated against a caller-supplied clock so tests can
/// control expiry deterministically instead of racing real time.
#[derive(Clone, Debug)]
pub struct CveFeedCacheEntry<Instant> {
    verdict: FeedVerdict,
    cached_at: Instant,
}

impl<Instant: Copy> CveFeedCacheEntry<Instant> {
    pub fn new(verdict: FeedVerdict, cached_at: Instant) -> Self {
        Self { verdict, cached_at }
    }

    pub fn verdict(&self) -> &FeedVerdict {
        &self.verdict
    }

    pub fn is_fresh<C>(&self, clock: &C, ttl: Duration) -> bool
    where
        C: GetNow<Instant = Instant> + GetElapsed,
    {
        clock.elapsed(self.cached_at) < ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::time::MockClock;

    #[test]
    fn fresh_within_ttl() {
        let clock = MockClock::new();
        let entry = CveFeedCacheEntry::new(FeedVerdict::Allow, clock.now());
        clock.advance(Duration::from_secs(30));
        assert!(entry.is_fresh(&clock, Duration::from_secs(60)));
    }

    #[test]
    fn stale_after_ttl() {
        let clock = MockClock::new();
        let entry = CveFeedCacheEntry::new(FeedVerdict::Allow, clock.now());
        clock.advance(Duration::from_secs(61));
        assert!(!entry.is_fresh(&clock, Duration::from_secs(60)));
    }

    #[test]
    fn boundary_at_exact_ttl_is_stale() {
        let clock = MockClock::new();
        let entry = CveFeedCacheEntry::new(FeedVerdict::Allow, clock.now());
        clock.advance(Duration::from_secs(60));
        assert!(!entry.is_fresh(&clock, Duration::from_secs(60)));
    }
}
