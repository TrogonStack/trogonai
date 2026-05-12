use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

pub const HEADER_RESOURCE: &str = "sentry-hook-resource";
pub const HEADER_SIGNATURE: &str = "sentry-hook-signature";
pub const HEADER_TIMESTAMP: &str = "sentry-hook-timestamp";
pub const HEADER_REQUEST_ID: &str = "request-id";

pub const NATS_HEADER_ACTION: &str = "X-Sentry-Action";
pub const NATS_HEADER_REQUEST_ID: &str = "X-Sentry-Request-Id";
pub const NATS_HEADER_RESOURCE: &str = "X-Sentry-Hook-Resource";
pub const NATS_HEADER_TIMESTAMP: &str = "X-Sentry-Hook-Timestamp";
