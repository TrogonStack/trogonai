use std::time::Duration;

pub(super) const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Writing out or mapping guest memory takes longer than a configuration call.
pub(super) const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(120);
