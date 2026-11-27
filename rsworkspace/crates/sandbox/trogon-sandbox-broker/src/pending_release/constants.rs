use std::time::Duration;

/// How long a marker survives in KV when nothing clears it. Shorter than the
/// quota receipt retention, so a redo always finds the receipt of a refund
/// that already landed.
pub const PENDING_RELEASE_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);
