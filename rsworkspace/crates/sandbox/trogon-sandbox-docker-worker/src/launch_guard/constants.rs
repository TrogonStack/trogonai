use std::time::Duration;

/// How long a fresh container is watched for an immediate exit.
pub(super) const SETTLE_WINDOW: Duration = Duration::from_millis(300);
pub(super) const SETTLE_POLL: Duration = Duration::from_millis(50);
/// How often a launched container is re-inspected for a late exit.
pub(crate) const WATCH_POLL: Duration = Duration::from_secs(2);
