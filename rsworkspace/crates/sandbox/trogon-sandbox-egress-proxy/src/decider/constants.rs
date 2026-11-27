use std::time::Duration;

pub const DEFAULT_DECIDE_TIMEOUT: Duration = Duration::from_secs(5);

pub const DEFAULT_MAX_IN_FLIGHT: usize = 16;

pub const MAX_REPLY_BYTES: usize = 1024;

pub const POLICY_PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);
