use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(25)).unwrap();

pub const HEADER_EVENT: &str = "x-gitlab-event";
pub const HEADER_WEBHOOK_ID: &str = "webhook-id";
pub const HEADER_WEBHOOK_TIMESTAMP: &str = "webhook-timestamp";
pub const HEADER_WEBHOOK_SIGNATURE: &str = "webhook-signature";
pub const HEADER_WEBHOOK_UUID: &str = "x-gitlab-webhook-uuid";
pub const HEADER_EVENT_UUID: &str = "x-gitlab-event-uuid";
pub const HEADER_IDEMPOTENCY_KEY: &str = "idempotency-key";
pub const HEADER_INSTANCE: &str = "x-gitlab-instance";

pub const NATS_HEADER_EVENT: &str = "X-GitLab-Event";
pub const NATS_HEADER_WEBHOOK_UUID: &str = "X-GitLab-Webhook-UUID";
pub const NATS_HEADER_EVENT_UUID: &str = "X-GitLab-Event-UUID";
pub const NATS_HEADER_INSTANCE: &str = "X-GitLab-Instance";
pub const NATS_HEADER_REJECT_REASON: &str = "X-GitLab-Reject-Reason";
