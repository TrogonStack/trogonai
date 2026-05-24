//! NATS auth-callout wire format (server `$SYS.REQ.USER.AUTH` path).
//!
//! Pinned against NATS server **2.10.x** (see `docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md`).

mod bridge_adapter;
mod callout_auth_response_claims;
mod nkey_public;
mod nkey_seed;
mod xkey_public;
mod server_auth_request_claims;
mod server_auth_request_envelope;
mod wire_codec;

#[cfg(test)]
pub(crate) mod test_encode;

pub use callout_auth_response_claims::CalloutAuthResponseClaims;
pub use nkey_public::NkeyPublic;
pub use nkey_seed::NkeySeed;
pub use xkey_public::XkeyPublic;
pub use server_auth_request_claims::ServerAuthRequestClaims;
pub use server_auth_request_envelope::ServerAuthRequestEnvelope;
pub use wire_codec::AuthCalloutWireCodec;

/// JWT `aud` on authorization **request** claims (`nats-server` `AuthRequestSubject`).
pub const AUTH_REQUEST_AUDIENCE: &str = "nats-authorization-request";

/// NATS message header carrying the server one-time XKey public key when encryption is enabled.
pub const AUTH_REQUEST_XKEY_HEADER: &str = "Nats-Server-Xkey";

/// Prefix of an encoded NATS JWT payload (before optional XKey encryption).
pub const NATS_JWT_PREFIX: &[u8] = b"eyJ";
