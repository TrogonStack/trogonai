use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HEADER_SIGNATURE: &str = "X-Notion-Signature";

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

pub const NATS_HEADER_ATTEMPT_NUMBER: &str = "X-Notion-Attempt-Number";
pub const NATS_HEADER_EVENT_ID: &str = "X-Notion-Event-Id";
pub const NATS_HEADER_EVENT_TYPE: &str = "X-Notion-Event-Type";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Notion-Reject-Reason";
pub const NATS_HEADER_SUBSCRIPTION_ID: &str = "X-Notion-Subscription-Id";
