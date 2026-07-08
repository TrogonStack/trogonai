//! Agent-side token exchange with a Person Server, per draft "Person Server"
//! (Token Endpoint, User Interaction, Clarification Chat) and "Deferred
//! Responses".
//!
//! Split into a sans-io core ([`core`]) implementing the "Deferred Response
//! State Machine" as a pure transition function, and a thin `reqwest` driver
//! ([`client`]) that performs the actual HTTP calls this crate's tests
//! exercise against `wiremock`. [`challenge`] holds the agent-side "Resource
//! Access and Resource Tokens" checks run on a `401`/`402` challenge.
//!
//! ## Presenting an auth token: NATS deviation
//!
//! Once [`client::PsTokenClient::request_token`] (or a resumed poll/
//! clarification call) yields [`client::ExchangeOutcome::Granted`], the
//! draft's "Auth Token Usage" says to present the auth token via
//! `Signature-Key` on subsequent requests to the resource, in place of the
//! agent token. **Trogon's NATS binding does not do this swap.** NATS PoP
//! headers ([`crate::signer::AgentSigner::sign_nats_request`]) are a
//! Trogon-defined envelope, not literal RFC 9421 `Signature-Key` -- there is
//! no header to "swap". The agent's identity key keeps signing every NATS
//! request via `agent_jwt`, and the auth token rides alongside as a distinct
//! proof of resource authorization in the separate `AAuth-Auth-Token` header.
//!
//! So "presenting" a granted auth token for NATS traffic is:
//!
//! ```ignore
//! let outcome = client.request_token(&request).await;
//! if let ExchangeOutcome::Granted(grant) = outcome {
//!     let signer = signer.with_auth_token(grant.auth_token);
//!     // `signer` now attaches AAuth-Auth-Token to every sign_nats_request call,
//!     // while agent_jwt keeps signing as the PoP credential.
//! }
//! ```
//!
//! This module never introduces a competing presentation mechanism for the
//! NATS path -- its job ends at handing back the auth token string.

pub mod challenge;
pub mod client;
pub mod core;

pub use challenge::{ChallengeVerifyError, verify_resource_challenge_claims, verify_resource_challenge_with_jwks};
pub use client::{ExchangeError, ExchangeOutcome, HttpRequestSigner, PsTokenClient, SignatureKeyOnlyHttpSigner};
pub use core::{ExchangeAction, ExchangeEvent, ExchangeState, PendingHeaders, TerminalReason, step};

use trogon_identity_types::aauth::person_server::TokenRequest;

use crate::subagent::{SubAgentError, parent_agent_of};

/// Builds a [`TokenRequest`] for the ordinary (non-sub-agent) agent token
/// request path, per "Agent Token Request", optionally attaching an upstream
/// auth token for call chaining per "Agent Delegation -- Call Chaining".
///
/// `agent_jwt` is the signer's own agent token (the one that will sign this
/// request via [`HttpRequestSigner`]) -- checked client-side for the
/// "Single-Level Depth" rule that a sub-agent's own token MUST NOT be used to
/// request its own authorization; use [`crate::subagent::build_subagent_token_request`]
/// instead if `agent_jwt` names a sub-agent. `upstream_token`, when supplied,
/// is attached as `TokenRequest.upstream_token` per "Call chaining" -- the
/// intermediary always signs with its own key, never the upstream token,
/// which is why signing is handled entirely by [`HttpRequestSigner`] and
/// this function only builds the body.
pub fn build_token_request(
    agent_jwt: &str,
    resource_token: impl Into<String>,
    upstream_token: Option<String>,
) -> Result<TokenRequest, SubAgentError> {
    if parent_agent_of(agent_jwt).is_some() {
        return Err(SubAgentError::ParentIsSubAgent);
    }
    Ok(TokenRequest {
        resource_token: resource_token.into(),
        upstream_token,
        ..TokenRequest::default()
    })
}

/// Determines which PS/AS to route a downstream token request to when acting
/// as an intermediary, per "Agent Delegation -- Call Chaining": "It
/// determines where to route the downstream token request from the upstream
/// auth token's `mission.approver` (if present) or `iss`."
///
/// `AuthClaims` (the auth token's claim set) has no `mission` field in
/// `trogon-identity-types` today -- only `ResourceClaims` carries
/// `mission: Option<MissionRef>`. This is flagged for promotion (see this
/// crate's delivery report); until then, callers that have a mission
/// approver available through another channel (e.g. carried alongside the
/// auth token in their own bookkeeping) pass it as `mission_approver`, and
/// this function prefers it over `iss` per the draft's fallback order.
#[must_use]
pub fn route_for_call_chaining<'a>(iss: &'a str, mission_approver: Option<&'a str>) -> &'a str {
    mission_approver.unwrap_or(iss)
}

#[cfg(test)]
mod tests;
