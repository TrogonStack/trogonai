use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::LeaseLifetimeError;
use crate::api::{ApiError, MAX_TIMEOUT_SECS};

/// How long a lease may keep its sandbox running.
#[derive(Clone, Copy, Debug)]
pub(crate) enum LeaseLifetime {
    Requested(u32),
    NativeDeadline(SystemTime),
}

impl LeaseLifetime {
    pub(crate) fn from_native(end_at: &str) -> Result<Self, LeaseLifetimeError> {
        let date = chrono::DateTime::parse_from_rfc3339(end_at).map_err(|_| LeaseLifetimeError::Invalid)?;
        let seconds = u64::try_from(date.timestamp()).map_err(|_| LeaseLifetimeError::Expired)?;
        let deadline = UNIX_EPOCH
            .checked_add(Duration::new(seconds, date.timestamp_subsec_nanos()))
            .ok_or(LeaseLifetimeError::OutOfRange)?;
        let lifetime = Self::NativeDeadline(deadline);
        lifetime.remaining_seconds()?;
        Ok(lifetime)
    }

    pub(crate) fn remaining_seconds(self) -> Result<u32, ApiError> {
        match self {
            Self::Requested(seconds) => Ok(seconds.clamp(1, MAX_TIMEOUT_SECS)),
            Self::NativeDeadline(deadline) => deadline
                .duration_since(SystemTime::now())
                .ok()
                .map(|span| span.as_secs())
                .filter(|seconds| (1..=u64::from(MAX_TIMEOUT_SECS)).contains(seconds))
                .and_then(|seconds| u32::try_from(seconds).ok())
                .ok_or_else(|| ApiError::Conflict("native lifetime is expired or unsupported".into())),
        }
    }
}
