use std::time::Duration;

use bytesize::ByteSize;

pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_SUBJECT_PREFIX: &str = "github";
pub const DEFAULT_STREAM_NAME: &str = "GITHUB";
pub const DEFAULT_STREAM_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7 days
pub const DEFAULT_NATS_ACK_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_MAX_BODY_SIZE: ByteSize = ByteSize::mib(25);
pub const DEFAULT_NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub const HEADER_SIGNATURE: &str = "x-hub-signature-256";
pub const HEADER_EVENT: &str = "x-github-event";
pub const HEADER_DELIVERY: &str = "x-github-delivery";

pub const NATS_HEADER_EVENT: &str = "X-GitHub-Event";
pub const NATS_HEADER_DELIVERY: &str = "X-GitHub-Delivery";
