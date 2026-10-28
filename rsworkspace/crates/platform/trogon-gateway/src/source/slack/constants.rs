use std::time::Duration;

use trogon_std::{ByteSize, HttpBodySizeMax};

pub const DEFAULT_HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(4)).unwrap();

pub const HEADER_SIGNATURE: &str = "x-slack-signature";
pub const HEADER_TIMESTAMP: &str = "x-slack-request-timestamp";
pub const HEADER_RETRY_NUM: &str = "x-slack-retry-num";
pub const HEADER_RETRY_REASON: &str = "x-slack-retry-reason";

pub const UNKNOWN_TEAM_ID: &str = "unknown";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Slack-Event-Type";
pub const NATS_HEADER_TEAM_ID: &str = "X-Slack-Team-Id";
pub const NATS_HEADER_EVENT_ID: &str = "X-Slack-Event-Id";
pub const NATS_HEADER_PAYLOAD_KIND: &str = "X-Slack-Payload-Kind";
pub const NATS_HEADER_RETRY_NUM: &str = "X-Slack-Retry-Num";
pub const NATS_HEADER_RETRY_REASON: &str = "X-Slack-Retry-Reason";
pub const NATS_HEADER_ENTERPRISE_ID: &str = "X-Slack-Enterprise-Id";
pub const NATS_HEADER_IS_ENTERPRISE_INSTALL: &str = "X-Slack-Is-Enterprise-Install";
pub const NATS_HEADER_API_APP_ID: &str = "X-Slack-Api-App-Id";
pub const NATS_HEADER_EVENT_SUBTYPE: &str = "X-Slack-Event-Subtype";
pub const NATS_HEADER_CHANNEL_TYPE: &str = "X-Slack-Channel-Type";
pub const NATS_HEADER_ENVELOPE_ID: &str = "X-Slack-Envelope-Id";
pub const NATS_HEADER_REQUEST_TIMESTAMP: &str = "X-Slack-Request-Timestamp";

pub const NATS_HEADER_TRANSPORT: &str = "X-Slack-Transport";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Slack-Reject-Reason";
pub const NATS_HEADER_CONTENT_TYPE: &str = "Content-Type";

pub const CONTENT_TYPE_FORM: &str = "application/x-www-form-urlencoded";
pub const CONTENT_TYPE_JSON: &str = "application/json";
pub const CONTENT_TYPE_UNKNOWN: &str = "application/octet-stream";

pub const DEFAULT_API_BASE_URL: &str = "https://slack.com/api/";
pub const APPS_CONNECTIONS_OPEN_METHOD: &str = "apps.connections.open";
pub const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(1);
pub const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

pub const SLACK_ACK_DEADLINE: Duration = Duration::from_secs(3);
pub const ACK_RESPONSE_MARGIN: Duration = Duration::from_millis(500);
pub const ACK_LIMIT_MAX: Duration = SLACK_ACK_DEADLINE.saturating_sub(ACK_RESPONSE_MARGIN);
