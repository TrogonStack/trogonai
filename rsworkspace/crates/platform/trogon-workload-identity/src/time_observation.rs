use std::time::{SystemTime, SystemTimeError};

use tokio::time::Instant;

pub(crate) struct TimeObservation {
    wall: SystemTime,
    monotonic: Instant,
}

impl TimeObservation {
    pub fn now() -> Self {
        Self::with_clock(SystemTime::now)
    }

    fn with_clock(read_wall: impl FnOnce() -> SystemTime) -> Self {
        let monotonic = Instant::now();
        let wall = read_wall();
        Self { wall, monotonic }
    }

    pub fn wall(&self) -> SystemTime {
        self.wall
    }

    pub fn deadline(&self, expires_at: SystemTime) -> Result<Instant, SystemTimeError> {
        Ok(self.monotonic + expires_at.duration_since(self.wall)?)
    }
}

#[cfg(test)]
mod tests;
