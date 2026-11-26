use std::time::Duration;

/// A wait the caller opted into. Neither upstream SDK range-checks its timeouts, so any duration is accepted,
/// including zero, which expires at once like Python's `anyio.fail_after(0)`. A deadline too far away to represent
/// never fires. [`Timeout::checked`] keeps the stricter positive, at most 24 hours check.
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
    /// The bound [`Timeout::checked`] enforces.
    pub const MAX: Duration = Duration::from_secs(24 * 60 * 60);

    pub const fn new(value: Duration) -> Self {
        Self(value)
    }

    /// Rejects zero and anything over [`Timeout::MAX`], for callers that want a misconfigured wait to fail early.
    pub fn checked(value: Duration) -> Result<Self, TimeoutError> {
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

/// Python's 60-second control request wait, which its session store options reuse.
impl Default for Timeout {
    fn default() -> Self {
        Self(Duration::from_secs(60))
    }
}

impl From<Duration> for Timeout {
    fn from(value: Duration) -> Self {
        Self::new(value)
    }
}
