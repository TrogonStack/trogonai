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
pub const MIN_SERVER_INFO_POLL_INTERVAL: Duration = Duration::from_millis(1);

pub const REQ_ID_HEADER: &str = "X-Req-Id";

pub const MAX_NATS_TOKEN_LENGTH: usize = 128;

pub const HEADER_CLAIM_CHECK: &str = "Trogon-Claim-Check";
pub const HEADER_CLAIM_BUCKET: &str = "Trogon-Claim-Bucket";
pub const HEADER_CLAIM_KEY: &str = "Trogon-Claim-Key";

pub(crate) const CLAIM_CHECK_VERSION: &str = "v1";
pub(crate) const PROTOCOL_OVERHEAD: usize = 8 * 1024;
pub(crate) const CLAIM_HEADER_PREFIX: &str = "Trogon-Claim-";
