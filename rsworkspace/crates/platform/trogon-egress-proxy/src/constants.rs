use std::time::Duration;

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
