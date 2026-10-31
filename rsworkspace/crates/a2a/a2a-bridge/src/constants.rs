pub(crate) const AGENT_ID_HEADER: &str = "x-a2a-agent-id";

pub const ENV_AUTH_CALLOUT_XKEY_PUBLIC: &str = "AUTH_CALLOUT_XKEY_PUBLIC";

/// JSON-RPC 2.0 reserved code for a server-side failure.
pub(crate) const INTERNAL_ERROR: i32 = -32603;

/// JSON-RPC protocol version every envelope this bridge emits carries.
pub(crate) const JSONRPC_VERSION: &str = "2.0";

/// Queue depth between a JetStream consumer task and the SSE response body it
/// feeds. The bound is what makes a slow HTTP client stall the consumer before
/// it acks, so unread events stay in JetStream instead of in this process.
pub(crate) const SSE_EVENT_QUEUE_CAPACITY: usize = 128;

/// How the HTTP edge treats AAuth proof of possession: `enforce`, the default,
/// `shadow` or `off`.
pub const ENV_AAUTH_MODE: &str = "A2A_BRIDGE_AAUTH_MODE";

/// Development opt-in, separate from [`ENV_AAUTH_MODE`], that every mode below
/// `enforce` has to be accompanied by (ADR#0082 Decision 4).
pub const ENV_AAUTH_DEV_UNENFORCED: &str = "A2A_BRIDGE_AAUTH_DEV_UNENFORCED";

/// Static JWKS file, a JSON object mapping each trusted issuer to its key set.
pub const ENV_AAUTH_JWKS_PATH: &str = "A2A_BRIDGE_AAUTH_JWKS_PATH";

/// `true` resolves issuer keys through well-known discovery instead of a file.
pub const ENV_AAUTH_JWKS_DISCOVERY: &str = "A2A_BRIDGE_AAUTH_JWKS_DISCOVERY";

/// How long a discovered key set is cached.
pub const ENV_AAUTH_JWKS_TTL_SECS: &str = "A2A_BRIDGE_AAUTH_JWKS_TTL_SECS";

/// Comma-separated issuers discovery may fetch keys for.
pub const ENV_AAUTH_JWKS_ALLOWED_ISSUERS: &str = "A2A_BRIDGE_AAUTH_JWKS_ALLOWED_ISSUERS";

/// The edge's own resource identifier: the `aud` an auth token has to name and
/// the issuer of the nonces the edge hands out.
pub const ENV_AAUTH_RESOURCE_ISS: &str = "A2A_BRIDGE_AAUTH_RESOURCE_ISS";

/// The authority clients reach the edge at, which every signature covers as
/// `@authority`.
pub const ENV_AAUTH_AUTHORITY: &str = "A2A_BRIDGE_AAUTH_AUTHORITY";

/// PKCS#8 PEM P-256 private key the edge signs its server nonces with.
pub const ENV_AAUTH_CHALLENGE_KEY_PATH: &str = "A2A_BRIDGE_AAUTH_CHALLENGE_KEY_PATH";

/// Key id the edge names its challenge key by.
pub const ENV_AAUTH_CHALLENGE_KID: &str = "A2A_BRIDGE_AAUTH_CHALLENGE_KID";

/// `required` makes every request bind a nonce the edge issued; `optional`,
/// the default, accepts one without demanding it (ADR#0051 Decision 3).
pub const ENV_AAUTH_NONCE: &str = "A2A_BRIDGE_AAUTH_NONCE";

/// How long a server nonce the edge issues stays usable.
pub const ENV_AAUTH_NONCE_TTL_SECS: &str = "A2A_BRIDGE_AAUTH_NONCE_TTL_SECS";

/// Longest span the edge accepts between a signature's `created` and
/// `expires`.
pub const ENV_AAUTH_SIGNATURE_VALIDITY_SECS: &str = "A2A_BRIDGE_AAUTH_SIGNATURE_VALIDITY_SECS";

/// Clock skew the edge tolerates on either side of a signature's window.
pub const ENV_AAUTH_MAX_SKEW_SECS: &str = "A2A_BRIDGE_AAUTH_MAX_SKEW_SECS";

/// How far past `exp`, or ahead of `iat`, a presented token is still accepted.
pub const ENV_AAUTH_LEEWAY_SECS: &str = "A2A_BRIDGE_AAUTH_LEEWAY_SECS";

/// Values of [`ENV_AAUTH_DEV_UNENFORCED`] that grant the opt-in, compared
/// without regard to ASCII case, the way `NATS_TLS_DISABLED` is read.
pub(crate) const DEV_UNENFORCED_VALUES: [&str; 3] = ["true", "1", "yes"];

/// Challenge for a request that carries no bearer credential (RFC 6750).
pub(crate) const WWW_AUTHENTICATE_BEARER: &str = "Bearer";

/// Challenge for a bearer credential the edge cannot bind to the verified
/// caller (RFC 6750 Section 3.1).
pub(crate) const WWW_AUTHENTICATE_INVALID_BEARER: &str = "Bearer error=\"invalid_token\"";
