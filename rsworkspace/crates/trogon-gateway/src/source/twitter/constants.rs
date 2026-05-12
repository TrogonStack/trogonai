use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

pub const HEADER_SIGNATURE: &str = "x-twitter-webhooks-signature";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Twitter-Event-Type";
pub const NATS_HEADER_PAYLOAD_KIND: &str = "X-Twitter-Payload-Kind";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Twitter-Reject-Reason";
