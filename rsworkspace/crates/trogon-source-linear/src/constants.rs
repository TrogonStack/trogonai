use std::time::Duration;

pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_SUBJECT_PREFIX: &str = "linear";
pub const DEFAULT_STREAM_NAME: &str = "LINEAR";
pub const DEFAULT_STREAM_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60; // 7 days
/// Default replay-attack tolerance: 60 seconds (as recommended by Linear).
pub const DEFAULT_TIMESTAMP_TOLERANCE_SECS: u64 = 60;
/// Default JetStream ACK timeout: 10 seconds.
pub const DEFAULT_NATS_ACK_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub const NATS_HEADER_REJECT_REASON: &str = "X-Linear-Reject-Reason";
