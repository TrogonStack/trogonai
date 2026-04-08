use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(4)).unwrap();

pub const HEADER_SIGNATURE: &str = "x-signature-ed25519";
pub const HEADER_TIMESTAMP: &str = "x-signature-timestamp";

pub const NATS_HEADER_INTERACTION_TYPE: &str = "X-Discord-Interaction-Type";
pub const NATS_HEADER_INTERACTION_ID: &str = "X-Discord-Interaction-Id";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Discord-Reject-Reason";
pub const NATS_HEADER_PAYLOAD_KIND: &str = "X-Discord-Payload-Kind";

pub const NATS_HEADER_EVENT_NAME: &str = "X-Discord-Event-Name";
pub const NATS_HEADER_GUILD_ID: &str = "X-Discord-Guild-Id";
