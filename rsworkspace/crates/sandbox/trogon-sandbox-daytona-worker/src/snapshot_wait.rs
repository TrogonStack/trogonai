use std::time::Duration;

use crate::constants::{DEFAULT_SNAPSHOT_POLL_INTERVAL, DEFAULT_SNAPSHOT_TIMEOUT};

/// How often a native snapshot is looked at, and how long it may take to turn active before the capture is reported
/// as not finished.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct SnapshotWait {
    pub poll_interval: Duration,
    pub timeout: Duration,
}

impl Default for SnapshotWait {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_SNAPSHOT_POLL_INTERVAL,
            timeout: DEFAULT_SNAPSHOT_TIMEOUT,
        }
    }
}
