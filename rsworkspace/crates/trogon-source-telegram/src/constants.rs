use std::time::Duration;

use bytesize::ByteSize;

pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_SUBJECT_PREFIX: &str = "telegram";
pub const DEFAULT_STREAM_NAME: &str = "TELEGRAM";
pub const DEFAULT_STREAM_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const DEFAULT_NATS_ACK_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_MAX_BODY_SIZE: ByteSize = ByteSize::mib(10);
