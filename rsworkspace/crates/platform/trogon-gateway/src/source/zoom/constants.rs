use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

pub const HEADER_SIGNATURE: &str = "x-zm-signature";
pub const HEADER_TIMESTAMP: &str = "x-zm-request-timestamp";

pub const EVENT_URL_VALIDATION: &str = "endpoint.url_validation";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Zoom-Event-Type";
pub const NATS_HEADER_REQUEST_TIMESTAMP: &str = "X-Zoom-Request-Timestamp";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Zoom-Reject-Reason";
