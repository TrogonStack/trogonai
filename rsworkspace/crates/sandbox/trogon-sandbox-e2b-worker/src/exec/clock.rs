use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::Instant;

use super::deadline_error::DeadlineError;

/// A finite deadline measured twice: against the wall clock so it survives a snapshot, and
/// against the monotonic clock so a wall clock step cannot extend it.
#[derive(Debug, Clone)]
pub(super) struct Clock {
    pub(super) issued_unix_ms: u64,
    pub(super) expires_unix_ms: u64,
    pub(super) due: Instant,
}

impl Clock {
    pub(super) fn now_unix_ms() -> Result<u64, DeadlineError> {
        Self::unix_ms_at(SystemTime::now())
    }

    pub(super) fn unix_ms_at(time: SystemTime) -> Result<u64, DeadlineError> {
        Self::millis(time.duration_since(UNIX_EPOCH)?)
    }

    pub(super) fn millis(duration: Duration) -> Result<u64, DeadlineError> {
        Ok(u64::try_from(duration.as_millis())?)
    }

    pub(super) fn new(seconds: u32) -> Result<Self, DeadlineError> {
        let budget = Duration::from_secs(u64::from(seconds.max(1)));
        let issued_unix_ms = Self::now_unix_ms()?;
        let expires_unix_ms = issued_unix_ms
            .checked_add(Self::millis(budget)?)
            .ok_or(DeadlineError::ExpiryOverflow)?;
        Ok(Self {
            issued_unix_ms,
            expires_unix_ms,
            due: Instant::now() + budget,
        })
    }

    pub(super) fn restore(
        issued_unix_ms: u64,
        expires_unix_ms: u64,
        captured_unix_ms: u64,
        remaining_ms: u64,
    ) -> Result<Self, DeadlineError> {
        let now = Self::now_unix_ms()?;
        if now < captured_unix_ms || now < issued_unix_ms {
            return Err(DeadlineError::BeforeCapture);
        }
        let remaining = remaining_ms.min(expires_unix_ms.saturating_sub(now));
        Ok(Self {
            issued_unix_ms,
            expires_unix_ms,
            due: Instant::now() + Duration::from_millis(remaining),
        })
    }

    pub(super) fn remaining_ms(&self) -> Result<u64, DeadlineError> {
        let now = Self::now_unix_ms()?;
        if now < self.issued_unix_ms {
            return Err(DeadlineError::BeforeIssuance);
        }
        let monotonic = Self::millis(self.due.saturating_duration_since(Instant::now()))?;
        Ok(monotonic.min(self.expires_unix_ms.saturating_sub(now)))
    }
}
