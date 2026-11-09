use std::num::TryFromIntError;
use std::time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialExpiry(i64);

impl CredentialExpiry {
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, CredentialExpiryError> {
        if seconds <= 0 {
            return Err(CredentialExpiryError::InvalidTimestamp);
        }
        Ok(Self(seconds))
    }

    pub fn unix_seconds(self) -> i64 {
        self.0
    }

    pub(crate) fn bounded_ttl(
        self,
        issued_at: SystemTime,
        maximum: Duration,
    ) -> Result<Duration, CredentialExpiryError> {
        let now = issued_at
            .duration_since(UNIX_EPOCH)
            .map_err(CredentialExpiryError::ClockBeforeEpoch)?;
        let now = i64::try_from(now.as_secs()).map_err(CredentialExpiryError::ClockOutOfRange)?;
        let remaining = self.0.saturating_sub(now);
        if remaining <= 0 {
            return Err(CredentialExpiryError::Expired);
        }
        let seconds = maximum
            .as_secs()
            .min(u64::try_from(remaining).map_err(CredentialExpiryError::ClockOutOfRange)?);
        if seconds == 0 {
            return Err(CredentialExpiryError::EmptyLifetime);
        }
        Ok(Duration::from_secs(seconds))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialExpiryError {
    #[error("credential expiry is missing or outside the supported timestamp range")]
    InvalidTimestamp,
    #[error("credential expired before broker admission")]
    Expired,
    #[error("broker admission lifetime must be at least one second")]
    EmptyLifetime,
    #[error("broker admission clock predates the Unix epoch")]
    ClockBeforeEpoch(#[source] SystemTimeError),
    #[error("broker admission clock is outside the supported timestamp range")]
    ClockOutOfRange(#[source] TryFromIntError),
}
