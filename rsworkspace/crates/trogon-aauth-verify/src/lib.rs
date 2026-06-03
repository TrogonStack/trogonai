//! AAuth verifier: parses `aa-agent+jwt` / `aa-auth+jwt`, validates JWT signature
//! against a pluggable JWKS resolver, and verifies proof-of-possession (PoP) for
//! HTTP (RFC 9421 subset) and NATS (Trogon-defined envelope mirroring RFC 9421).
//!
//! Server-side only. The agent-side signer lives in `trogon-aauth-sdk`.

#![allow(clippy::module_name_repetitions)]

pub mod challenge;
pub mod jkt;
pub mod jwks;
pub mod nats_pop;
pub mod replay;
pub mod time_source;
pub mod token;

pub use challenge::{ChallengeMinter, mint_resource_jwt};
pub use jkt::jwk_thumbprint;
pub use jwks::{JwksResolver, StaticJwks};
pub use nats_pop::{NatsHeaders, NatsPopVerifier, NatsRequest};
pub use replay::{InMemoryReplayStore, ReplayStore};
pub use time_source::{SystemTimeSource, TimeSource};
pub use token::{TokenError, TokenVerifier, VerifiedAgent, VerifiedAuth, VerifiedResource};

/// Errors returned by AAuth verification.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("token: {0}")]
    Token(#[from] TokenError),
    #[error("pop: {0}")]
    Pop(String),
    #[error("replay: {0}")]
    Replay(String),
    #[error("policy: {0}")]
    Policy(String),
}
