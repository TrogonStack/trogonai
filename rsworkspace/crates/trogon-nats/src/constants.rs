use std::time::Duration;

pub const ENV_NATS_URL: &str = "NATS_URL";
pub const ENV_NATS_CREDS: &str = "NATS_CREDS";
pub const ENV_NATS_NKEY: &str = "NATS_NKEY";
pub const ENV_NATS_USER: &str = "NATS_USER";
pub const ENV_NATS_PASSWORD: &str = "NATS_PASSWORD";
pub const ENV_NATS_TOKEN: &str = "NATS_TOKEN";
pub const DEFAULT_NATS_URL: &str = "localhost:4222";

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

pub const REQ_ID_HEADER: &str = "X-Req-Id";
pub const PAYLOAD_PREVIEW_BYTES: usize = 16;

pub const MAX_NATS_TOKEN_LENGTH: usize = 128;
