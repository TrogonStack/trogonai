use std::time::Duration;

use super::ReleaseTimeoutError;
use super::constants::{DEFAULT_RELEASE_TIMEOUT, MAX_RELEASE_TIMEOUT};

/// How long one release call waits for its answer, from the request going
/// out to the reply coming back.
///
/// Defaults to [`DEFAULT_RELEASE_TIMEOUT`]. A wait of zero or longer than
/// [`MAX_RELEASE_TIMEOUT`] is refused, never clamped, so a configuration that
/// asks for one is caught where it is read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseTimeout(Duration);

impl ReleaseTimeout {
    pub fn new(timeout: Duration) -> Result<Self, ReleaseTimeoutError> {
        if timeout.is_zero() {
            return Err(ReleaseTimeoutError::Zero);
        }
        if timeout > MAX_RELEASE_TIMEOUT {
            return Err(ReleaseTimeoutError::TooLong {
                requested: timeout,
                max: MAX_RELEASE_TIMEOUT,
            });
        }
        Ok(Self(timeout))
    }

    pub const fn get(self) -> Duration {
        self.0
    }
}

impl Default for ReleaseTimeout {
    fn default() -> Self {
        Self(DEFAULT_RELEASE_TIMEOUT)
    }
}

#[cfg(test)]
mod tests;
