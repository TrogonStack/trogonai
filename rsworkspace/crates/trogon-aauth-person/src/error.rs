//! Typed errors for the Person Server, mapped to draft wire error codes at the
//! HTTP boundary (`crate::http`). See "Token Endpoint Error Codes"
//! (#token-endpoint-error-codes), "Polling Error Codes" (#polling-error-codes),
//! "Interaction Endpoint Errors" (#interaction-endpoint-errors), and "Mission
//! Status Errors" (#mission-status-errors).

use trogon_aauth_verify::TokenError;
use trogon_identity_types::aauth::error::{InteractionEndpointError, MissionStatus, PollingError, TokenEndpointError};

/// Failures verifying the inbound token-endpoint request before policy runs,
/// per "Agent Token Request" and "Resource Challenge Verification"
/// (#resource-challenge-verification).
#[derive(Debug, thiserror::Error)]
pub enum RequestVerificationError {
    #[error("invalid agent token: {0}")]
    AgentToken(#[source] TokenError),
    #[error("invalid resource token: {0}")]
    ResourceToken(#[source] TokenError),
    #[error("invalid subagent token: {0}")]
    SubagentToken(#[source] TokenError),
    #[error("invalid upstream token: {0}")]
    UpstreamToken(#[source] TokenError),
    /// "PS Response": three-party mode requires the resource token's `aud`
    /// to identify this PS.
    #[error("resource_token aud does not identify this PS (expected {expected}, got {actual})")]
    ResourceTokenWrongAudience { expected: String, actual: String },
    /// "Resource Token Verification" step 6 (direct case): `agent_jkt` must
    /// match the presenting agent's confirmation key.
    #[error("resource_token agent_jkt does not match the presenting agent's cnf.jwk")]
    ResourceTokenAgentKeyMismatch,
    /// "Parent-Mediated Authorization" step 3: for a sub-agent request,
    /// `resource_token.agent_jkt` must match the *subagent* token's `cnf.jwk`.
    #[error("resource_token agent_jkt does not match subagent_token cnf.jwk")]
    ResourceTokenSubagentKeyMismatch,
    /// "Parent-Mediated Authorization" step 3: `resource_token.agent` must
    /// name the sub-agent, not the parent.
    #[error("resource_token agent does not name the sub-agent")]
    ResourceTokenSubagentIdentifierMismatch,
    /// "Single-Level Depth": a sub-agent token itself carrying `parent_agent`
    /// cannot be re-delegated.
    #[error("subagent_token must not itself be a sub-agent (single-level depth)")]
    SubagentTokenIsItselfSubagent,
    /// "Single-Level Depth": a sub-agent's own token must not sign a token
    /// request directly.
    #[error("agent_token is a sub-agent token; sub-agents must not request authorization directly")]
    AgentTokenIsSubagent,
    /// "Parent-Mediated Authorization" step 3: `subagent_token.parent_agent`
    /// must name the signing (parent) agent.
    #[error("subagent_token parent_agent ({parent_agent}) does not match signing agent ({signing_agent})")]
    SubagentParentMismatch {
        parent_agent: String,
        signing_agent: String,
    },
    /// "Upstream Token Verification" step 3: the upstream token's `aud` must
    /// equal the `iss` of the intermediary's own agent token.
    #[error("upstream_token aud ({upstream_aud}) does not match intermediary agent_token iss ({agent_token_iss})")]
    UpstreamAudienceBindingMismatch {
        upstream_aud: String,
        agent_token_iss: String,
    },
    /// The HTTP request did not carry a `Signature-Key` presenting an agent
    /// token, per "Agent Token Request".
    #[error("missing Signature-Key agent token presentation")]
    MissingSignatureKey,
}

/// Failures minting an `aa-auth+jwt`, per "Auth Token Structure" (#auth-tokens).
#[derive(Debug, thiserror::Error)]
pub enum MintError {
    #[error("encode: {0}")]
    Encode(#[source] jsonwebtoken::errors::Error),
    #[error("ttl overflowed i64 when added to iat ({iat} + {ttl_secs})")]
    TtlOverflow { iat: i64, ttl_secs: i64 },
}

/// Failures operating on a pending request, per "Clarification Chat" and
/// "Deferred Responses" (#deferred-responses).
#[derive(Debug, thiserror::Error)]
pub enum PendingRequestError {
    #[error("no pending request for id {0}")]
    NotFound(String),
    /// "Cancel Request": subsequent requests to a canceled pending URL return
    /// `410 Gone`.
    #[error("pending request {0} is gone (canceled or already resolved)")]
    Gone(String),
    /// "Clarification Limits": PSes SHOULD enforce a maximum round count.
    #[error("clarification round limit ({limit}) exceeded for pending request {0}", limit = .1)]
    ClarificationLimitExceeded(String, u32),
    /// "Updated Request": the new resource token MUST have the same `iss`,
    /// `agent`, and `agent_jkt` as the original.
    #[error("updated_request resource_token does not match original iss/agent/agent_jkt")]
    UpdatedRequestIdentityMismatch,
    #[error("request verification failed: {0}")]
    Verification(#[from] RequestVerificationError),
    #[error("mint failed: {0}")]
    Mint(#[from] MintError),
}

/// Top-level Person Server error for one token-endpoint evaluation, mapped to
/// [`TokenEndpointError`] at the HTTP boundary.
#[derive(Debug, thiserror::Error)]
pub enum PersonServerError {
    #[error("request verification failed: {0}")]
    Verification(#[from] RequestVerificationError),
    #[error("mint failed: {0}")]
    Mint(#[from] MintError),
    #[error("pending request error: {0}")]
    Pending(#[from] PendingRequestError),
    /// "Token Endpoint Error Codes": `user_unreachable` is terminal -- no
    /// channel to reach the user and the agent lacks the `interaction`
    /// capability.
    #[error("no channel to reach the user and agent lacks interaction capability")]
    UserUnreachable,
    /// "Mission Status Errors": the referenced mission is no longer active.
    #[error("mission {0} is not active (status: {1:?})")]
    MissionNotActive(String, MissionStatus),
    #[error("mission {0} not found")]
    MissionNotFound(String),
    #[error("interaction relay error: {0}")]
    Interaction(#[from] InteractionRelayError),
    /// Person policy denied the request outright, per "PS Response".
    #[error("denied: {0}")]
    Denied(String),
}

/// Failures relaying an interaction to the user, per "Interaction Endpoint"
/// (#interaction-endpoint) and "Interaction Endpoint Errors".
#[derive(Debug, thiserror::Error)]
pub enum InteractionRelayError {
    /// "Interaction Endpoint Errors": non-terminal, the PS has no channel
    /// available right now; the agent falls back to directing the user itself.
    #[error("no channel available to relay this interaction")]
    Unavailable,
    /// Terminal: the PS cannot reach the user and the agent does not declare
    /// the `interaction` capability.
    #[error("user unreachable and agent lacks interaction capability")]
    UserUnreachable,
}

impl PersonServerError {
    /// Maps this error to the draft's `error` wire code where a direct
    /// mapping exists. The draft's error tables are not exhaustive over every
    /// internal failure mode this crate can produce, so callers needing a
    /// wire response for a variant with no direct mapping should treat `None`
    /// as `server_error` per "Token Endpoint Error Codes".
    #[must_use]
    pub fn token_endpoint_code(&self) -> TokenEndpointError {
        match self {
            PersonServerError::Verification(RequestVerificationError::AgentToken(_))
            | PersonServerError::Verification(RequestVerificationError::SubagentToken(_))
            | PersonServerError::Verification(RequestVerificationError::AgentTokenIsSubagent)
            | PersonServerError::Verification(RequestVerificationError::SubagentTokenIsItselfSubagent)
            | PersonServerError::Verification(RequestVerificationError::SubagentParentMismatch { .. })
            | PersonServerError::Verification(RequestVerificationError::MissingSignatureKey) => {
                TokenEndpointError::InvalidAgentToken
            }
            PersonServerError::Verification(RequestVerificationError::ResourceToken(TokenError::Expired)) => {
                TokenEndpointError::ExpiredResourceToken
            }
            PersonServerError::Verification(RequestVerificationError::ResourceToken(_))
            | PersonServerError::Verification(RequestVerificationError::ResourceTokenWrongAudience { .. })
            | PersonServerError::Verification(RequestVerificationError::ResourceTokenAgentKeyMismatch)
            | PersonServerError::Verification(RequestVerificationError::ResourceTokenSubagentKeyMismatch)
            | PersonServerError::Verification(RequestVerificationError::ResourceTokenSubagentIdentifierMismatch) => {
                TokenEndpointError::InvalidResourceToken
            }
            PersonServerError::Verification(RequestVerificationError::UpstreamToken(_))
            | PersonServerError::Verification(RequestVerificationError::UpstreamAudienceBindingMismatch { .. }) => {
                TokenEndpointError::InvalidRequest
            }
            PersonServerError::UserUnreachable => TokenEndpointError::UserUnreachable,
            _ => TokenEndpointError::ServerError,
        }
    }

    /// HTTP status for this error. For [`PersonServerError::Denied`] this
    /// follows "Polling Error Codes" `denied` (403) since a synchronous
    /// person-policy denial is the same outcome a poller sees for a denied
    /// pending request, per "PS Response": "The agent MUST NOT proceed."
    /// All other variants map through [`Self::token_endpoint_code`] per
    /// "Token Endpoint Error Codes", which defines no status table beyond
    /// the per-code status column mirrored here.
    #[must_use]
    pub fn http_status(&self) -> u16 {
        if matches!(self, PersonServerError::Denied(_)) {
            return 403;
        }
        match self.token_endpoint_code() {
            TokenEndpointError::InvalidRequest
            | TokenEndpointError::InvalidAgentToken
            | TokenEndpointError::ExpiredAgentToken
            | TokenEndpointError::InvalidResourceToken
            | TokenEndpointError::ExpiredResourceToken => 400,
            TokenEndpointError::UserUnreachable => 403,
            TokenEndpointError::ServerError => 500,
        }
    }

    /// Wire `error` code for this error's JSON body. [`Self::token_endpoint_code`]
    /// only covers the "Token Endpoint Error Codes" table; [`PersonServerError::Denied`]
    /// uses the "Polling Error Codes" `denied` code instead since it is not a
    /// member of that table.
    #[must_use]
    pub fn wire_code(&self) -> String {
        match self {
            PersonServerError::Denied(_) => "denied".to_string(),
            other => serde_json::to_value(other.token_endpoint_code())
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "server_error".to_string()),
        }
    }
}

/// Maps a [`PollingError`] to its HTTP status per "Polling Error Codes".
#[must_use]
pub fn polling_error_status(err: &PollingError) -> u16 {
    match err {
        PollingError::Denied | PollingError::Abandoned => 403,
        PollingError::Expired => 408,
        PollingError::InvalidCode => 410,
        PollingError::SlowDown => 429,
        PollingError::ServerError => 500,
    }
}

/// Maps an [`InteractionEndpointError`] to its HTTP status per "Interaction
/// Endpoint Errors".
#[must_use]
pub fn interaction_endpoint_error_status(err: &InteractionEndpointError) -> u16 {
    match err {
        InteractionEndpointError::InteractionUnavailable => 424,
    }
}

#[cfg(test)]
mod tests;
