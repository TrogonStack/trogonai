use std::time::Duration;

/// How often the worker republishes its capacity.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
