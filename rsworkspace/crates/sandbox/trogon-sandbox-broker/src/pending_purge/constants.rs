use std::time::Duration;

/// How long a marker survives in KV when nothing clears it. Bounds how long
/// an old lease id can still reach a name that a newer lease may reuse.
pub const PENDING_PURGE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
