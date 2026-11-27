use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::clock_error::SnapshotClockError;

pub(crate) fn now_ms() -> Result<u64, SnapshotClockError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis().try_into()?)
}

/// The Unix time in milliseconds `duration` from now, saturating instead of overflowing.
pub(crate) fn after_ms(duration: Duration) -> Result<u64, SnapshotClockError> {
    Ok(now_ms()?.saturating_add(duration.as_millis().try_into().unwrap_or(u64::MAX)))
}
