//! Sub-agent support per draft "Sub-Agents": single-level depth enforcement
//! and the parent-mediated token request builder.
//!
//! `AgentClaims` in `trogon-identity-types` has no `parent_agent` field
//! (only `iss, sub, jti, iat, exp, dwk, cnf, ps`) -- that crate is owned
//! elsewhere and is not this SDK's to extend. `AgentClaims` does not set
//! `#[serde(deny_unknown_fields)]`, so a wire JWT minted by a server this SDK
//! doesn't own can carry `parent_agent` fine; it is just silently dropped if
//! decoded through `AgentClaims`. This module therefore decodes the raw JWT
//! payload itself (base64url + `serde_json::Value`, unverified) to read
//! `parent_agent` when present, the same pattern
//! `trogon-aauth-verify::token`'s private `iss_of` helper uses for `iss`.
//! This is a structural, non-cryptographic read: minting and verifying
//! sub-agent tokens is the agent-provider's / verify crate's job, not this
//! SDK's, and is out of scope here.
//!
//! `parent_agent` on `AgentClaims` is flagged for promotion to
//! `trogon-identity-types` in this crate's delivery report; until then this
//! module is the SDK-local home for sub-agent bookkeeping that needs it.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use trogon_identity_types::aauth::person_server::TokenRequest;

/// Errors from sub-agent request building and depth checks.
#[derive(Debug, thiserror::Error)]
pub enum SubAgentError {
    /// The parent signer's own agent token already has a `parent_agent`
    /// claim, i.e. the "parent" is itself a sub-agent. Per "Single-Level
    /// Depth", a sub-agent cannot mediate another sub-agent's authorization
    /// (that would create depth 2). Client-side fast-fail mirroring the PS's
    /// mandatory server-side rejection.
    #[error("parent agent token has its own parent_agent claim; cannot mediate as it is itself a sub-agent")]
    ParentIsSubAgent,
    /// The prospective parent's agent token already has a `parent_agent`
    /// claim, so minting a sub-agent under it would create depth 2.
    #[error("prospective parent agent token already has a parent_agent claim; minting under it would create depth 2")]
    ParentAlreadySubAgent,
    /// The token passed as `subagent_agent_jwt` has no `parent_agent` claim,
    /// so it is not a sub-agent token and the caller used the wrong function.
    #[error("supplied agent token has no parent_agent claim; it is not a sub-agent token")]
    NotASubAgentToken,
    /// The agent token's payload segment could not be decoded as JSON.
    #[error("agent token payload could not be decoded")]
    MalformedAgentToken,
}

/// Reads the `parent_agent` claim from an agent token's payload without
/// verifying the token's signature. Signature verification is out of scope
/// for this SDK; this is a structural read only, used for client-side
/// sub-agent bookkeeping before a signed request is even built.
#[must_use]
pub fn parent_agent_of(agent_jwt: &str) -> Option<String> {
    decode_payload(agent_jwt).ok().and_then(|payload| payload.parent_agent)
}

#[derive(Deserialize)]
struct ParentAgentOnly {
    #[serde(default)]
    parent_agent: Option<String>,
}

fn decode_payload(jwt: &str) -> Result<ParentAgentOnly, SubAgentError> {
    let mut parts = jwt.splitn(3, '.');
    let _ = parts.next();
    let payload_b64 = parts.next().ok_or(SubAgentError::MalformedAgentToken)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|_| SubAgentError::MalformedAgentToken)?;
    serde_json::from_slice(&payload).map_err(|_| SubAgentError::MalformedAgentToken)
}

/// Given a *prospective* parent's own agent token, checks whether minting a
/// sub-agent token under it would violate "Single-Level Depth": an agent
/// provider MUST NOT issue a sub-agent token whose parent is itself a
/// sub-agent. This is a pure client-side pre-check for an orchestrator
/// embedding this SDK that wants to fail fast before asking its agent
/// provider to mint a sub-agent token; the agent provider still owns the
/// authoritative minting-side enforcement.
pub fn can_mint_subagent_under(prospective_parent_agent_jwt: &str) -> Result<(), SubAgentError> {
    match decode_payload(prospective_parent_agent_jwt)?.parent_agent {
        Some(_) => Err(SubAgentError::ParentAlreadySubAgent),
        None => Ok(()),
    }
}

/// Builds the [`TokenRequest`] a parent agent sends to the PS's
/// `token_endpoint` to mediate authorization on behalf of one of its
/// sub-agents, per "Parent-Mediated Authorization".
///
/// The parent signs this request with its OWN key (via its own
/// [`crate::signer::AgentSigner`], applied by the caller when actually
/// sending the request -- this function only builds the body). `subagent_resource_token`
/// is the resource token the sub-agent obtained on its own, bound to its own
/// key, and passed to the parent out of band (e.g. IPC, not implemented
/// here). `subagent_agent_jwt` is the sub-agent's own `aa-agent+jwt`.
///
/// Fails fast, client-side, on two "Single-Level Depth" violations that the
/// PS would otherwise have to reject:
/// - `parent_agent_jwt` itself has a `parent_agent` claim (this "parent" is
///   itself a sub-agent and mediating further would create depth 2):
///   [`SubAgentError::ParentIsSubAgent`].
/// - `subagent_agent_jwt` has no `parent_agent` claim at all, meaning it is
///   not actually a sub-agent token: [`SubAgentError::NotASubAgentToken`].
pub fn build_subagent_token_request(
    parent_agent_jwt: &str,
    subagent_resource_token: impl Into<String>,
    subagent_agent_jwt: &str,
) -> Result<TokenRequest, SubAgentError> {
    if parent_agent_of(parent_agent_jwt).is_some() {
        return Err(SubAgentError::ParentIsSubAgent);
    }
    if parent_agent_of(subagent_agent_jwt).is_none() {
        return Err(SubAgentError::NotASubAgentToken);
    }

    Ok(TokenRequest {
        resource_token: subagent_resource_token.into(),
        subagent_token: Some(subagent_agent_jwt.to_string()),
        ..TokenRequest::default()
    })
}

#[cfg(test)]
mod tests;
