//! AAuth agent-side signer: mints the `aa-agent+jwt` PoP header set for NATS
//! requests (Trogon-defined envelope mirroring RFC 9421), and parses
//! `AAuth-Requirement` deny replies into a typed next step.
//!
//! Agent-side only. The server-side verifier lives in `trogon-aauth-verify`;
//! that crate's `NatsPopVerifier` is what this crate's signatures must
//! satisfy, and `jwk_thumbprint` is re-exported from there so both sides
//! compute the same `jkt` from the same algorithm.
//!
//! Beyond signing, this crate also implements the agent side of the Person
//! Server token exchange (module [`exchange`]): the "Deferred Response State
//! Machine", resource-challenge verification, and auth-token response
//! verification. See [`exchange`]'s module docs for how a granted auth token
//! is handed off to [`AgentSigner::with_auth_token`] -- Trogon's NATS binding
//! deviates from the draft's literal `Signature-Key` swap, and that deviation
//! is documented there in detail.

pub mod capabilities;
pub mod delegation;
pub mod error;
pub mod exchange;
pub mod signer;
pub mod subagent;
pub mod verify_response;

pub use capabilities::{CapabilitiesHeaderExt, capabilities_header};
pub use delegation::ActChainExt;
pub use error::AgentSignerError;
pub use exchange::{
    ChallengeVerifyError, ExchangeError, ExchangeOutcome, HttpRequestSigner, PsTokenClient, SignatureKeyOnlyHttpSigner,
    build_token_request, route_for_call_chaining, verify_resource_challenge_claims,
    verify_resource_challenge_with_jwks,
};
pub use signer::{AgentSigner, PopHeaders};
pub use subagent::{SubAgentError, build_subagent_token_request, can_mint_subagent_under, parent_agent_of};
pub use trogon_aauth_verify::jwk_thumbprint;
pub use trogon_identity_types::aauth::Requirement;
pub use verify_response::{
    VerifiedAuthResponse, VerifyResponseError, verify_auth_claims, verify_auth_token_response_with_jwks,
};

/// Parse the value of an `AAuth-Requirement` header into a typed next step.
///
/// Thin wrapper over [`trogon_identity_types::aauth::Requirement::parse`] so
/// callers of this crate don't need a direct dependency on
/// `trogon-identity-types` just to react to a -32118 deny.
#[must_use]
pub fn parse_requirement(header_value: &str) -> Requirement {
    Requirement::parse(header_value)
}

#[cfg(test)]
mod tests;
