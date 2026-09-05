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

const _: () = assert!(!VERSION_CURRENT.is_empty());
const _: () = assert!(!VERSION_PREVIOUS.is_empty());

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

/// Signature algorithms an inbound OIDC ID token may assert.
///
/// Scoped to the RSA family because [`crate::credentials::oidc`] only builds
/// decoding keys from RSA JWK components; an allowlist that admitted anything
/// else would name algorithms that cannot verify here anyway. The list exists
/// so the algorithm is a deployment decision rather than something the token
/// under verification chooses for itself.
pub(crate) const OIDC_ALLOWED_ALGORITHMS: [jsonwebtoken::Algorithm; 6] = [
    jsonwebtoken::Algorithm::RS256,
    jsonwebtoken::Algorithm::RS384,
    jsonwebtoken::Algorithm::RS512,
    jsonwebtoken::Algorithm::PS256,
    jsonwebtoken::Algorithm::PS384,
    jsonwebtoken::Algorithm::PS512,
];
