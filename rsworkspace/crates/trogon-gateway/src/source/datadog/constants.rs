use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

/// Default HTTP header carrying the shared secret token the operator configures
/// as a custom header on the Datadog webhook. Datadog does not sign webhook
/// requests, so this header is the only authentication signal. Operators may
/// override the header name via `webhook_token_header`.
pub const DEFAULT_HEADER_WEBHOOK_TOKEN: &str = "x-datadog-webhook-token";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Datadog-Event-Type";
pub const NATS_HEADER_EVENT_ID: &str = "X-Datadog-Event-Id";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Datadog-Reject-Reason";
