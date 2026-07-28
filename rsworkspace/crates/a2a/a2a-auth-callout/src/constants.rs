//! Crate-wide constants for a2a-auth-callout.

use std::time::Duration;

/// Header name carrying a serialized [`crate::caller_jwt_header::CallerJwtHeaderValue`] on
/// every A2A request, including gateway-mediated traffic.
pub const CALLER_JWT_HEADER_NAME: &str = "A2a-Caller-Jwt";

pub(crate) const MAX_LEN: usize = 256;

pub(crate) const HEADER_TYPE: &str = "JWT";
pub(crate) const HEADER_ALGORITHM: &str = "ed25519-nkey";

pub(crate) const VERSION_CURRENT: &str = "current";
pub(crate) const VERSION_PREVIOUS: &str = "previous";

/// NATS subject the server uses for auth callout requests.
pub(crate) const AUTH_CALLOUT_SUBJECT: &str = "$SYS.REQ.USER.AUTH";

pub(crate) const DEFAULT_DENIAL_TTL: Duration = Duration::from_secs(60);

/// JWT `aud` on authorization **request** claims (`nats-server` `AuthRequestSubject`).
pub const AUTH_REQUEST_AUDIENCE: &str = "nats-authorization-request";

/// NATS message header carrying the server one-time XKey public key when encryption is enabled.
pub const AUTH_REQUEST_XKEY_HEADER: &str = "Nats-Server-Xkey";

/// Prefix of an encoded NATS JWT payload (before optional XKey encryption).
pub const NATS_JWT_PREFIX: &[u8] = b"eyJ";

/// Default minted user JWT TTL, in seconds, for the `a2a-auth-callout` binary.
pub const DEFAULT_USER_JWT_TTL_SECS: u64 = 300;
