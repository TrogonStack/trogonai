use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, or zero for a time before it.
pub(crate) fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}
