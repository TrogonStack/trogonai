#[cfg(any(test, feature = "test-support"))]
use std::sync::{Arc, Mutex};
#[cfg(any(test, feature = "test-support"))]
use std::time::Duration;

#[cfg(any(test, feature = "test-support"))]
use super::{EpochClock, GetElapsed, GetNow};

/// Fixed wall-clock time source for deterministic tests.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
pub struct FixedEpochClock(pub std::time::SystemTime);

#[cfg(any(test, feature = "test-support"))]
impl FixedEpochClock {
    pub fn from_secs(secs: u64) -> Self {
        Self(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
    }
}

#[cfg(any(test, feature = "test-support"))]
impl EpochClock for FixedEpochClock {
    fn system_time(&self) -> std::time::SystemTime {
        self.0
    }
}

/// Time only advances when you call [`advance`](MockClock::advance) or
/// [`set`](MockClock::set), eliminating flakiness from real-time
/// measurements.
///
/// # Examples
///
/// ```ignore
/// use trogon_std::time::{GetNow, GetElapsed, MockClock};
/// use std::time::Duration;
///
/// let clock = MockClock::new();
/// let t0 = clock.now();
///
/// clock.advance(Duration::from_millis(500));
/// assert_eq!(clock.elapsed(t0), Duration::from_millis(500));
///
/// clock.advance(Duration::from_millis(500));
/// assert_eq!(clock.elapsed(t0), Duration::from_secs(1));
/// ```
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
pub struct MockClock {
    current: Arc<Mutex<Duration>>,
}

#[cfg(any(test, feature = "test-support"))]
impl MockClock {
    /// Starts at time zero.
    pub fn new() -> Self {
        Self {
            current: Arc::new(Mutex::new(Duration::ZERO)),
        }
    }

    pub fn advance(&self, duration: Duration) {
        let mut current = self.current.lock().unwrap();
        *current += duration;
    }

    /// Set the absolute time (vs. relative [`advance`](Self::advance)).
    pub fn set(&self, duration: Duration) {
        let mut current = self.current.lock().unwrap();
        *current = duration;
    }

    pub fn current_time(&self) -> Duration {
        *self.current.lock().unwrap()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

/// A `Duration` offset from an arbitrary epoch — the instant type
/// used by [`MockClock`].
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MockInstant(pub Duration);

#[cfg(any(test, feature = "test-support"))]
impl GetNow for MockClock {
    type Instant = MockInstant;

    fn now(&self) -> MockInstant {
        MockInstant(*self.current.lock().unwrap())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl GetElapsed for MockClock {
    fn elapsed(&self, since: MockInstant) -> Duration {
        let now = *self.current.lock().unwrap();
        now.saturating_sub(since.0)
    }
}

#[cfg(test)]
mod tests;
