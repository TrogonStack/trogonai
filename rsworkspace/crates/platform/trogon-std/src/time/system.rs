use std::time::Duration;

use super::{EpochClock, GetElapsed, GetNow};

/// Zero-sized type — delegates to `std::time::Instant`.
#[derive(Clone)]
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

impl EpochClock for SystemClock {
    #[inline]
    fn system_time(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }
}

#[cfg(test)]
mod tests;
