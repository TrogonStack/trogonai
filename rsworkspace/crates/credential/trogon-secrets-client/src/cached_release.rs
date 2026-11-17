use std::time::{Duration, SystemTime};

use trogon_std::time::GetElapsed;

use super::ReleasedValue;

/// One release a client holds, and when it stops serving it.
///
/// It stops at the earlier of two deadlines set from one hold: `expires_at`
/// on the wall clock, and `lifetime` after `stored_at` on the monotonic
/// clock, so a wall clock stepped back cannot stretch the hold.
#[derive(Clone, Debug)]
pub(crate) struct CachedRelease<I> {
    value: ReleasedValue,
    expires_at: SystemTime,
    stored_at: I,
    lifetime: Duration,
}

impl<I: Copy> CachedRelease<I> {
    pub(crate) const fn new(value: ReleasedValue, expires_at: SystemTime, stored_at: I, lifetime: Duration) -> Self {
        Self {
            value,
            expires_at,
            stored_at,
            lifetime,
        }
    }

    pub(crate) const fn value(&self) -> &ReleasedValue {
        &self.value
    }

    /// Whether it still serves at `now` on the wall clock and at `clock`'s
    /// reading of the monotonic one.
    pub(crate) fn is_live_at<C>(&self, now: SystemTime, clock: &C) -> bool
    where
        C: GetElapsed<Instant = I>,
    {
        now < self.expires_at && clock.elapsed(self.stored_at) < self.lifetime
    }
}
