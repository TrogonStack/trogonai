use std::time::Duration;

/// Rust host wait policy, capped at 24 hours with a 60-second default.
/// The shutdown default is 5 seconds; these are not upstream SDK restrictions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeout(Duration);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TimeoutError {
    #[error("timeout must be positive")]
    Zero,
    #[error("timeout exceeds the library's 24-hour maximum")]
    TooLarge,
}

impl Timeout {
    pub const MAX: Duration = Duration::from_secs(24 * 60 * 60);
    pub const SHUTDOWN_DEFAULT: Self = Self(Duration::from_secs(5));

    pub fn new(value: Duration) -> Result<Self, TimeoutError> {
        if value.is_zero() {
            return Err(TimeoutError::Zero);
        }
        if value > Self::MAX {
            return Err(TimeoutError::TooLarge);
        }
        Ok(Self(value))
    }

    pub const fn as_duration(self) -> Duration {
        self.0
    }
}

impl Default for Timeout {
    fn default() -> Self {
        Self(Duration::from_secs(60))
    }
}

impl TryFrom<Duration> for Timeout {
    type Error = TimeoutError;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
