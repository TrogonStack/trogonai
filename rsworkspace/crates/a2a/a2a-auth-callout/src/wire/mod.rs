//! NATS auth-callout wire format (server `$SYS.REQ.USER.AUTH` path).
//!
//! Pinned against NATS server **2.14.x**.

mod bridge_adapter;
mod callout_auth_response_claims;
mod nkey_public;
mod nkey_seed;
mod server_auth_request_claims;
mod server_auth_request_envelope;
mod wire_codec;
mod xkey_public;

#[cfg(test)]
pub(crate) mod test_encode;

pub use callout_auth_response_claims::CalloutAuthResponseClaims;
pub use nkey_public::NkeyPublic;
pub use nkey_seed::NkeySeed;
pub use server_auth_request_claims::ServerAuthRequestClaims;
pub use server_auth_request_envelope::ServerAuthRequestEnvelope;
pub use wire_codec::AuthCalloutWireCodec;
pub use xkey_public::XkeyPublic;

pub use crate::constants::{AUTH_REQUEST_AUDIENCE, AUTH_REQUEST_XKEY_HEADER, NATS_JWT_PREFIX};
