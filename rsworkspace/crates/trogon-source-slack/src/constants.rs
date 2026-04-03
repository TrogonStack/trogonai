use std::time::Duration;

use bytesize::ByteSize;

pub const DEFAULT_PORT: u16 = 3000;
pub const DEFAULT_SUBJECT_PREFIX: &str = "slack";
pub const DEFAULT_STREAM_NAME: &str = "SLACK";
pub const DEFAULT_STREAM_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7 days
pub const DEFAULT_NATS_ACK_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_MAX_BODY_SIZE: ByteSize = ByteSize::mib(1);
pub const DEFAULT_NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_TIMESTAMP_MAX_DRIFT_SECS: u64 = 300; // 5 minutes

pub const HEADER_SIGNATURE: &str = "x-slack-signature";
pub const HEADER_TIMESTAMP: &str = "x-slack-request-timestamp";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Slack-Event-Type";
pub const NATS_HEADER_TEAM_ID: &str = "X-Slack-Team-Id";
pub const NATS_HEADER_EVENT_ID: &str = "X-Slack-Event-Id";
pub const NATS_HEADER_PAYLOAD_KIND: &str = "X-Slack-Payload-Kind";

pub const NATS_HEADER_REJECT_REASON: &str = "X-Slack-Reject-Reason";

pub const CONTENT_TYPE_FORM: &str = "application/x-www-form-urlencoded";
