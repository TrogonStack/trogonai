use std::time::Duration;

use bytesize::ByteSize;

pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_SUBJECT_PREFIX: &str = "discord";
pub const DEFAULT_STREAM_NAME: &str = "DISCORD";
pub const DEFAULT_STREAM_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const DEFAULT_NATS_ACK_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_MAX_BODY_SIZE: ByteSize = ByteSize::mib(4);
pub const DEFAULT_NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub const HEADER_SIGNATURE: &str = "x-signature-ed25519";
pub const HEADER_TIMESTAMP: &str = "x-signature-timestamp";

pub const NATS_HEADER_INTERACTION_TYPE: &str = "X-Discord-Interaction-Type";
pub const NATS_HEADER_INTERACTION_ID: &str = "X-Discord-Interaction-Id";
