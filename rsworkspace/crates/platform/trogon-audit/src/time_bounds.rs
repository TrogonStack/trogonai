use std::time::SystemTime;

/// The window a checkpoint's `issued_at` must fall in, both ends included.
///
/// The verifier owns no clock, so the caller states what counts as fresh: an
/// audit replays a past window, a monitor wants the last few intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeBounds {
    earliest: SystemTime,
    latest: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("checkpoint time bounds must start no later than they end")]
pub struct TimeBoundsError;

impl TimeBounds {
    pub fn new(earliest: SystemTime, latest: SystemTime) -> Result<Self, TimeBoundsError> {
        if earliest > latest {
            return Err(TimeBoundsError);
        }
        Ok(Self { earliest, latest })
    }

    pub fn earliest(&self) -> SystemTime {
        self.earliest
    }

    pub fn latest(&self) -> SystemTime {
        self.latest
    }

    pub fn contains(&self, time: SystemTime) -> bool {
        self.earliest <= time && time <= self.latest
    }
}

#[cfg(test)]
mod tests;
