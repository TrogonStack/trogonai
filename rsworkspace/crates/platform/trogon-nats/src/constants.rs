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

pub const REQ_ID_HEADER: &str = "Trogon-Req-Id";

pub const MAX_NATS_TOKEN_LENGTH: usize = 128;

pub const HEADER_CLAIM_CHECK: &str = "Trogon-Claim-Check";
pub const HEADER_CLAIM_BUCKET: &str = "Trogon-Claim-Bucket";
pub const HEADER_CLAIM_KEY: &str = "Trogon-Claim-Key";

/// Bucket a claim check writes to and reads from when nothing overrides it.
/// Whoever publishes and whoever consumes must name the same bucket, so the
/// default belongs to the protocol rather than to either side of it.
pub const DEFAULT_CLAIM_BUCKET: &str = "trogon-claims";

pub(crate) const CLAIM_CHECK_VERSION: &str = "v1";
pub(crate) const PROTOCOL_OVERHEAD: usize = 8 * 1024;
pub(crate) const CLAIM_HEADER_PREFIX: &str = "Trogon-Claim-";

/// ADR#0055 Limits: at most 16 tokens per subject.
///
/// The hard ceiling before NATS escapes to the heap is 32; 16 is the budget
/// this profile spends, leaving room for method arity and durable suffixes.
pub const MAX_SUBJECT_TOKENS: usize = 16;

/// ADR#0055 Limits: at most 256 bytes per subject.
pub const MAX_SUBJECT_BYTES: usize = 256;

/// The reserved token introducing ADR#0055's method-to-terminal escape encoding
/// (`custom.{base64url}`). The token following it is the sole exemption from
/// lower_snake; the wildcard, token-count, and byte limits still bind it.
pub const ESCAPE_TOKEN: &str = "custom";
