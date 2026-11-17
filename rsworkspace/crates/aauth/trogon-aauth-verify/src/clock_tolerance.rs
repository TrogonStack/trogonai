use crate::constants::{DEFAULT_CLOCK_SKEW_SECS, MAX_CLOCK_SKEW_SECS};

/// Clock difference a verifier tolerates between its own clock and the
/// signer's, applied as clock skew to a signature's window and as expiry
/// leeway to the tokens it presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockTolerance(u32);

impl ClockTolerance {
    /// # Errors
    ///
    /// [`ClockToleranceError`] when `secs` exceeds [`MAX_CLOCK_SKEW_SECS`].
    pub fn new(secs: u32) -> Result<Self, ClockToleranceError> {
        if secs > MAX_CLOCK_SKEW_SECS {
            return Err(ClockToleranceError { secs });
        }
        Ok(Self(secs))
    }

    #[must_use]
    pub fn secs(self) -> u32 {
        self.0
    }
}

impl Default for ClockTolerance {
    fn default() -> Self {
        Self(DEFAULT_CLOCK_SKEW_SECS)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("clock tolerance must be at most {max}s, got {secs}s", max = MAX_CLOCK_SKEW_SECS)]
pub struct ClockToleranceError {
    secs: u32,
}
