//! NATS auth-callout wire format (server `$SYS.REQ.USER.AUTH` path).
//!
//! Pinned against NATS server **2.14.x**.

mod auth_request_error;
mod bridge_adapter;
mod callout_auth_response_claims;
mod nkey_public;
mod server_auth_request_claims;
mod server_auth_request_envelope;
mod wire_codec;

#[cfg(test)]
pub(crate) mod test_encode;

pub use crate::admission_keys::XkeyPublic;
pub use auth_request_error::AuthRequestError;
pub use callout_auth_response_claims::CalloutAuthResponseClaims;
pub use nkey_public::NkeyPublic;
pub use server_auth_request_claims::ServerAuthRequestClaims;
pub use server_auth_request_envelope::ServerAuthRequestEnvelope;
pub use wire_codec::AuthCalloutWireCodec;

pub use crate::constants::{AUTH_REQUEST_AUDIENCE, AUTH_REQUEST_XKEY_HEADER, NATS_JWT_PREFIX};
