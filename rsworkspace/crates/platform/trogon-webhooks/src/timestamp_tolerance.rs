#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimestampTolerance(u64);

#[derive(Debug, thiserror::Error)]
#[error("timestamp tolerance must be between 1 and 3600 seconds")]
pub struct TimestampToleranceError;

impl TimestampTolerance {
    pub fn new(seconds: u64) -> Result<Self, TimestampToleranceError> {
        if !(1..=3600).contains(&seconds) {
            return Err(TimestampToleranceError);
        }
        Ok(Self(seconds))
    }

    pub fn as_secs(self) -> u64 {
        self.0
    }
}
