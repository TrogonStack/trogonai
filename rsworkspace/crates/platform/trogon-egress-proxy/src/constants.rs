use std::num::NonZeroUsize;
use std::time::Duration;

pub(crate) const DEFAULT_MAX_CONNECTIONS: NonZeroUsize = NonZeroUsize::MIN.saturating_add(127);
pub(crate) const MAX_CONNECTION_LIMIT: usize = 4096;

/// Longest agent identifier accepted, in bytes. It travels inside a URI SAN.
pub(crate) const AGENT_ID_MAX_LEN: usize = 128;

/// Longest trust domain accepted, in bytes (the SPIFFE limit).
pub(crate) const TRUST_DOMAIN_MAX_LEN: usize = 255;

/// Longest DNS name accepted, in bytes (RFC 1035).
pub(crate) const HOST_NAME_MAX_LEN: usize = 253;

/// Longest DNS label accepted, in bytes (RFC 1035).
pub(crate) const HOST_NAME_MAX_LABEL_LEN: usize = 63;

pub(crate) const SPIFFE_SCHEME_PREFIX: &str = "spiffe://";

pub(crate) const AGENT_PATH_PREFIX: &str = "agent/";

/// Smallest head limit that still fits a minimal `CONNECT` request.
pub(crate) const MIN_HEAD_BYTES: usize = 128;

pub(crate) const HEAD_TERMINATOR: &[u8] = b"\r\n\r\n";

pub(crate) const HEAD_READ_CHUNK: usize = 1024;

pub(crate) const CONNECTION_ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

/// Pause after a failed `accept` so a persistent error cannot spin the loop.
pub(crate) const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);

/// Response header that carries the fault code of a refusal.
pub(crate) const FAULT_HEADER: &str = "x-trogon-egress-fault";

/// Seconds a client is told to wait before retrying an operational refusal.
pub(crate) const RETRY_AFTER_SECONDS: u32 = 5;

/// Bytes in a TLS record header: type, legacy version, length.
pub(crate) const TLS_RECORD_HEADER_LEN: usize = 5;

/// `TLSPlaintext.length` may not exceed 2^14 (RFC 8446 section 5.1).
pub(crate) const TLS_MAX_RECORD_BODY_LEN: usize = 16_384;

pub(crate) const TLS_CONTENT_TYPE_HANDSHAKE: u8 = 22;

pub(crate) const TLS_HANDSHAKE_CLIENT_HELLO: u8 = 1;

pub(crate) const TLS_EXTENSION_SERVER_NAME: u16 = 0;

pub(crate) const TLS_NAME_TYPE_HOST_NAME: u8 = 0;

/// The `target` of a decision record whose request never named a destination
/// (a malformed head, or a method other than CONNECT). Not `host:port`
/// shaped, so no real destination can collide with it.
pub(crate) const NO_DESTINATION_TARGET: &str = "no_destination";

/// The `target` of a decision record whose destination is longer than an audit
/// name may be (a 250 byte host and a port do not fit). The decision is still
/// recorded; only the spelling of where is replaced.
pub(crate) const DESTINATION_TOO_LONG_TARGET: &str = "destination_too_long";

/// Log target of the one local line written per decision.
pub(crate) const AUDIT_LOG_TARGET: &str = "egress_audit";

/// Prefix of the writer id, ahead of the instance name.
pub(crate) const WRITER_ID_PREFIX: &str = "egress-proxy:";

/// The component token the proxy's integrity records are published under.
pub(crate) const AUDIT_COMPONENT: &str = "egress_proxy";

/// How long the audit connection may take to be built. NATS `connect` returns
/// before any server answers, so this bounds setup, not reachability.
pub(crate) const AUDIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const AUDIT_READINESS_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) const POLICY_RECHECK_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const DENIAL_AUDIT_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const MAX_SESSION_LIFETIME: Duration = Duration::from_secs(330);
pub(crate) const REGISTRATION_MAX_BYTES: usize = 256 * 1024;
pub(crate) const LIVE_POLICY_MAX_BUCKET_BYTES: usize = 64;
pub(crate) const LIVE_POLICY_MAX_BYTES: usize = 256 * 1024;
pub(crate) const LIVE_POLICY_MAX_STORE_BYTES: i64 = 1024 * 1024;
pub(crate) const LIVE_POLICY_MAX_AGENTS: usize = 1024;
pub(crate) const LIVE_POLICY_MAX_PERMITS: usize = 4096;
pub(crate) const LIVE_POLICY_MAX_LIFETIME: Duration = Duration::from_secs(325);
pub(crate) const LIVE_POLICY_POLL_INTERVAL: Duration = Duration::from_secs(1);
pub(crate) const LIVE_POLICY_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const KV_OPERATION_HEADER: &str = "KV-Operation";

#[cfg(test)]
pub(crate) const LIVE_POLICY_TEST_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
pub(crate) const LIVE_POLICY_TEST_POLL: Duration = Duration::from_millis(20);
#[cfg(test)]
pub(crate) const LIVE_POLICY_TUNNEL_TEST_WAIT: Duration = Duration::from_secs(8);
