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

impl TimeBounds {
    pub fn new(earliest: SystemTime, latest: SystemTime) -> Self {
        Self { earliest, latest }
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
