use std::time::Duration;

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

pub const APPS_CONNECTIONS_OPEN_URL: &str = "https://slack.com/api/apps.connections.open";
pub const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(1);
pub const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);
