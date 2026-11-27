use std::time::Duration;

pub const DEFAULT_MAX_EVENTS_PER_WINDOW: u32 = 20;

pub const DEFAULT_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(10);

pub const DEFAULT_MAX_TRACKED_LEASES: usize = 1024;
