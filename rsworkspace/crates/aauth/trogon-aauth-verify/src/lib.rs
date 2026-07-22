//! AAuth verifier: parses `aa-agent+jwt` / `aa-auth+jwt`, validates JWT signature
//! against a pluggable JWKS resolver, and verifies proof-of-possession (PoP) for
//! HTTP (RFC 9421 subset) and NATS (Trogon-defined envelope mirroring RFC 9421).
//!
//! Server-side only. The agent-side signer lives in `trogon-aauth-sdk`.
//!
//! Subsequent slice adds the NATS PoP verifier on top of this foundation.

#![allow(clippy::module_name_repetitions)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod challenge;
pub mod delegation;
pub mod http_pop;
pub mod jkt;
pub mod jwks;
pub mod jwks_cache;
pub mod jwks_http;
pub mod mission;
pub mod nats_pop;
pub mod replay;
pub mod time_source;
pub mod token;
pub mod upstream;

pub use challenge::{
    ChallengeMinter, ResourceChallengeContext, ResourceChallengeError, mint_resource_jwt, verify_resource_challenge,
};
pub use delegation::{
    DelegationError, FlattenedActEntry, MAX_CHAIN_DEPTH, flatten_act_chain, is_valid_agent_identifier, verify_act_chain,
};
pub use http_pop::{HttpPopError, HttpPopVerifier, HttpRequest, VerifiedAuthPresenter, VerifiedPresenter};
pub use jkt::jwk_thumbprint;
pub use jwks::{JwksError, JwksResolver, StaticJwks};
pub use jwks_cache::{CachedJwksResolver, DEFAULT_NEGATIVE_TTL_SECS, DEFAULT_TTL_SECS};
pub use jwks_http::{HttpJwksResolver, MaxResponseBytes, RequestTimeout, WellKnownDwk};
pub use mission::{MissionError, extract_mission_claim, verify_mission_blob_hash, verify_mission_header_matches_claim};
pub use nats_pop::{NatsHeaders, NatsPopError, NatsPopVerifier, NatsRequest};
pub use replay::{InMemoryReplayStore, ReplayStore};
pub use time_source::{SystemTimeSource, TimeSource};
pub use token::{
    InvalidKeyMaterialSource, RequestContextError, RequestSigningContext, TokenError, TokenVerifier, VerifiedAgent,
    VerifiedAuth, VerifiedResource,
};
pub use upstream::{UpstreamTokenError, UpstreamTokenRequest, UpstreamVerification, verify_upstream_token};

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

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
