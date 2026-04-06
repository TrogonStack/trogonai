use std::time::Duration;

use trogon_std::{ByteSize, HttpBodySizeMax};

pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_SUBJECT_PREFIX: &str = "gitlab";
pub const DEFAULT_STREAM_NAME: &str = "GITLAB";
pub const DEFAULT_STREAM_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7 days
pub const DEFAULT_NATS_ACK_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(25)).unwrap();

pub const HEADER_TOKEN: &str = "x-gitlab-token";
pub const HEADER_EVENT: &str = "x-gitlab-event";
pub const HEADER_WEBHOOK_UUID: &str = "x-gitlab-webhook-uuid";
pub const HEADER_EVENT_UUID: &str = "x-gitlab-event-uuid";
pub const HEADER_IDEMPOTENCY_KEY: &str = "idempotency-key";
pub const HEADER_INSTANCE: &str = "x-gitlab-instance";

pub const NATS_HEADER_EVENT: &str = "X-GitLab-Event";
pub const NATS_HEADER_WEBHOOK_UUID: &str = "X-GitLab-Webhook-UUID";
pub const NATS_HEADER_EVENT_UUID: &str = "X-GitLab-Event-UUID";
pub const NATS_HEADER_INSTANCE: &str = "X-GitLab-Instance";
pub const NATS_HEADER_REJECT_REASON: &str = "X-GitLab-Reject-Reason";
