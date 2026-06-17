//! Wire-level caller identity types for the A2A protocol.
//!
//! The A2A protocol carries caller identity in a JWT header on every request.
//! This crate defines the wire types those headers contain, kept separate from
//! the gateway-side machinery that mints/verifies them so transport crates
//! (`a2a-nats`, `a2a-nats-server`, etc.) can read identity off the wire without
//! pulling in signing key management or NATS auth-callout server logic.

pub mod caller;
pub mod error;
pub mod jwt;
pub mod principal;

pub use caller::CallerId;
pub use error::JwtError;
pub use jwt::{CALLER_JWT_HEADER_NAME, CallerJwtHeaderValue, MintedUserJwt, decode_nats_user_payload};
pub use principal::{SpiceDbPrincipal, SpiceDbSubject};
