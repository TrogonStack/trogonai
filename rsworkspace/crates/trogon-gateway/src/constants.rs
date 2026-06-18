use std::time::Duration;

pub const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const NATS_SERVER_INFO_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub const CLAIM_CHECK_BUCKET: &str = "trogon-claims";

pub const DEFAULT_STREAM_MAX_AGE_SECS: u64 = 604_800;
pub const DEFAULT_NATS_ACK_TIMEOUT_SECS: u64 = 10;
pub const DEFAULT_GITLAB_TIMESTAMP_TOLERANCE_SECS: u64 = 300;
pub const DEFAULT_SLACK_TIMESTAMP_MAX_DRIFT_SECS: u64 = 300;
pub const DEFAULT_LINEAR_TIMESTAMP_TOLERANCE_SECS: u64 = 60;
pub const DEFAULT_INCIDENTIO_TIMESTAMP_TOLERANCE_SECS: u64 = 300;
