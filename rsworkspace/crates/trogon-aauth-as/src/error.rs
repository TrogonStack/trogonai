//! Typed errors for AS request verification and token minting, per "AS
//! Token Endpoint" (#as-token-endpoint), "Auth Token Verification"
//! (#auth-token-verification), and "Upstream Token Verification"
//! (#upstream-token-verification). Mapped to wire error codes in
//! [`crate::http`] via "Token Endpoint Error Codes".

use trogon_aauth_verify::TokenError;

use crate::subagent::ParentAgentError;
use crate::trust::TrustRegistryError;

/// Failures while verifying a PS-to-AS token request before policy runs.
#[derive(Debug, thiserror::Error)]
pub enum RequestVerificationError {
    /// The requesting PS is not a trusted issuer, per "PS-AS Federation":
    /// the AS "discovers the AS's metadata ... and calls the AS's
    /// token_endpoint" only after establishing trust; an unrecognized PS
    /// must be rejected.
    #[error("untrusted PS: {0}")]
    UntrustedPs(#[source] TrustRegistryError),
    #[error("invalid resource_token: {0}")]
    ResourceToken(#[source] TokenError),
    /// "PS-to-AS Token Request": the resource token's `aud` MUST identify
    /// this AS.
    #[error("resource_token aud does not identify this AS (expected {expected}, got {actual})")]
    ResourceTokenWrongAudience { expected: String, actual: String },
    #[error("invalid agent_token: {0}")]
    AgentToken(#[source] TokenError),
    #[error("invalid subagent_token: {0}")]
    SubagentToken(#[source] TokenError),
    /// Reading `parent_agent` back off an already-verified `aa-agent+jwt`
    /// payload failed; see [`crate::subagent`] for why this is a separate
    /// decode step from `verify_agent`.
    #[error("could not read parent_agent claim: {0}")]
    MalformedParentAgentClaim(#[source] ParentAgentError),
    /// "PS-to-AS Token Request": for a sub-agent request, `subagent_token`'s
    /// `parent_agent` MUST match `agent_token`'s subject.
    #[error("subagent_token parent_agent ({parent_agent}) does not match agent_token subject ({agent_token_sub})")]
    SubagentParentMismatch {
        parent_agent: String,
        agent_token_sub: String,
    },
    /// "Parent-Mediated Authorization": `resource_token.agent_jkt` MUST match
    /// the *subagent* token's `cnf.jwk`, not the signing (parent) key, when a
    /// `subagent_token` is present.
    #[error("resource_token agent_jkt does not match subagent_token cnf.jwk")]
    ResourceTokenSubagentKeyMismatch,
    /// "Parent-Mediated Authorization": when no `subagent_token` is present,
    /// `resource_token.agent_jkt` MUST match the requesting agent's `cnf.jwk`.
    #[error("resource_token agent_jkt does not match agent_token cnf.jwk")]
    ResourceTokenAgentKeyMismatch,
    /// "Single-Level Depth": a sub-agent token itself carrying `parent_agent`
    /// cannot be re-delegated.
    #[error("subagent_token must not itself be a sub-agent (single-level depth)")]
    SubagentTokenIsItselfSubagent,
    /// "Single-Level Depth": a request signed by a sub-agent's own token
    /// (`agent_token` carries `parent_agent`) must be rejected -- only the
    /// parent may request authorization.
    #[error("agent_token is a sub-agent token; sub-agents must not request authorization directly")]
    AgentTokenIsSubagent,
    #[error("invalid upstream_token: {0}")]
    UpstreamToken(#[source] TokenError),
    /// "Upstream Token Verification" step 2: `iss` must be a trusted issuer.
    #[error("upstream_token issuer is not trusted: {0}")]
    UpstreamUntrustedIssuer(#[source] TrustRegistryError),
    /// "Upstream Token Verification" step 3: the upstream token's `aud` must
    /// equal the `iss` of the intermediary's own agent token.
    #[error("upstream_token aud ({upstream_aud}) does not match intermediary agent_token iss ({agent_token_iss})")]
    UpstreamAudienceBindingMismatch {
        upstream_aud: String,
        agent_token_iss: String,
    },
}

/// Failures while minting an auth token, per "Auth Token Structure".
#[derive(Debug, thiserror::Error)]
pub enum MintError {
    #[error("encode: {0}")]
    Encode(#[source] jsonwebtoken::errors::Error),
    #[error("ttl overflowed i64 when added to iat ({iat} + {ttl_secs})")]
    TtlOverflow { iat: i64, ttl_secs: i64 },
}

/// Top-level error for one AS token-endpoint evaluation, per "Token Endpoint
/// Error Codes" (#token-endpoint-error-codes).
#[derive(Debug, thiserror::Error)]
pub enum AccessServerError {
    #[error("request verification failed: {0}")]
    Verification(#[from] RequestVerificationError),
    #[error("mint failed: {0}")]
    Mint(#[from] MintError),
    #[error("policy denied: {reason}")]
    Denied { reason: String },
    #[error("no pending claims-required request for id {0}")]
    UnknownPendingRequest(String),
}

#[cfg(test)]
mod tests;
