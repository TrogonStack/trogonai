use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(1)).unwrap();

pub const HEADER_SIGNATURE: &str = "x-slack-signature";
pub const HEADER_TIMESTAMP: &str = "x-slack-request-timestamp";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Slack-Event-Type";
pub const NATS_HEADER_TEAM_ID: &str = "X-Slack-Team-Id";
pub const NATS_HEADER_EVENT_ID: &str = "X-Slack-Event-Id";
pub const NATS_HEADER_PAYLOAD_KIND: &str = "X-Slack-Payload-Kind";

pub const NATS_HEADER_REJECT_REASON: &str = "X-Slack-Reject-Reason";

pub const CONTENT_TYPE_FORM: &str = "application/x-www-form-urlencoded";
