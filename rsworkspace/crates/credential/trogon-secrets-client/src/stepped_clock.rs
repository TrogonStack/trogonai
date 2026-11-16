//! A clock the crate's unit tests move by hand.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use trogon_std::time::{EpochClock, GetElapsed, GetNow, MockClock, MockInstant};

/// A wall clock and a monotonic clock, starting at `wall` and zero, that move
/// only when a test moves them and need not move together.
#[derive(Clone)]
pub(crate) struct SteppedClock {
    wall: Arc<Mutex<SystemTime>>,
    monotonic: MockClock,
}

impl SteppedClock {
    pub(crate) fn at(wall: SystemTime) -> Self {
        Self {
            wall: Arc::new(Mutex::new(wall)),
            monotonic: MockClock::new(),
        }
    }

    /// Moves both clocks forward by `by`.
    pub(crate) fn advance(&self, by: Duration) {
        *self.wall.lock().unwrap_or_else(PoisonError::into_inner) += by;
        self.monotonic.advance(by);
    }

    /// Sets the wall clock to `wall`, leaving the monotonic clock where it is.
    pub(crate) fn set_wall(&self, wall: SystemTime) {
        *self.wall.lock().unwrap_or_else(PoisonError::into_inner) = wall;
    }
}

impl EpochClock for SteppedClock {
    fn system_time(&self) -> SystemTime {
        *self.wall.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl GetNow for SteppedClock {
    type Instant = MockInstant;

    fn now(&self) -> MockInstant {
        self.monotonic.now()
    }
}

impl GetElapsed for SteppedClock {
    fn elapsed(&self, since: MockInstant) -> Duration {
        self.monotonic.elapsed(since)
    }
}
