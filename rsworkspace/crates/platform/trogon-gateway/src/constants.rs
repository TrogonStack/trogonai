use std::time::Duration;

pub const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const NATS_SERVER_INFO_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub const CLAIM_CHECK_BUCKET: &str = "trogon-claims";
/// Grace window added to the longest configured stream retention when sizing the
/// claim-check bucket TTL, so a message at the edge of expiry can still resolve
/// its claim before the object is reclaimed.
pub const CLAIM_CHECK_TTL_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

pub const DEFAULT_STREAM_MAX_AGE_SECS: u64 = 604_800;
pub const DEFAULT_NATS_ACK_TIMEOUT_SECS: u64 = 10;
pub const DEFAULT_GITLAB_TIMESTAMP_TOLERANCE_SECS: u64 = 300;
pub const DEFAULT_SLACK_TIMESTAMP_MAX_DRIFT_SECS: u64 = 300;
pub const DEFAULT_LINEAR_TIMESTAMP_TOLERANCE_SECS: u64 = 60;
pub const DEFAULT_INCIDENTIO_TIMESTAMP_TOLERANCE_SECS: u64 = 300;
