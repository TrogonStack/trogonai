use trogon_std::{ByteSize, HttpBodySizeMax};

use crate::source::standard_webhooks::HeaderNames;

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

pub const HEADER_WEBHOOK_ID: &str = "webhook-id";
pub const HEADER_WEBHOOK_TIMESTAMP: &str = "webhook-timestamp";
pub const HEADER_WEBHOOK_SIGNATURE: &str = "webhook-signature";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Incidentio-Event-Type";
pub const NATS_HEADER_WEBHOOK_ID: &str = "X-Incidentio-Webhook-Id";
pub const NATS_HEADER_WEBHOOK_TIMESTAMP: &str = "X-Incidentio-Webhook-Timestamp";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Incidentio-Reject-Reason";

pub const HEADER_NAMES: HeaderNames = HeaderNames {
    webhook_id: HEADER_WEBHOOK_ID,
    webhook_timestamp: HEADER_WEBHOOK_TIMESTAMP,
    webhook_signature: HEADER_WEBHOOK_SIGNATURE,
};
