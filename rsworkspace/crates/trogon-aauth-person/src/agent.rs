//! Verifies the inbound token-endpoint request per "Agent Token Request"
//! (#agent-token-request) and "Resource Challenge Verification"
//! (#resource-challenge-verification), before person-policy runs.
//!
//! HTTP Message Signature verification of the outer request (RFC 9421,
//! `Signature-Key` / `Signature-Input` / `Signature`) is a transport concern
//! left to [`crate::http`] and not implemented by this crate yet -- see the
//! crate-level docs' "Deviations" note. This module verifies the JWTs carried
//! in the request body and the trust/binding relationships between them.

use trogon_aauth_verify::{JwksResolver, TimeSource, TokenVerifier, VerifiedAuth};
use trogon_identity_types::aauth::person_server::TokenRequest;
use trogon_identity_types::aauth::{AgentClaims, ResourceClaims};

use crate::error::RequestVerificationError;
use crate::mint::BindingAgent;
use crate::subagent::parent_agent_of;

/// Everything verified out of one [`TokenRequest`], before policy runs.
#[derive(Debug)]
pub struct VerifiedRequest {
    pub resource_claims: ResourceClaims,
    pub agent_claims: AgentClaims,
    pub subagent_claims: Option<AgentClaims>,
    pub upstream: Option<VerifiedAuth>,
    pub binding: BindingAgent,
    pub justification: Option<String>,
}

/// Verifies an agent's token request against this Person Server's own
/// issuer identity (`ps_iss`), per "PS Response" (three-party mode: the
/// resource token's `aud` must identify this PS).
pub async fn verify_request<R, C>(
    verifier: &TokenVerifier<R, C>,
    ps_iss: &str,
    agent_token: &str,
    req: &TokenRequest,
) -> Result<VerifiedRequest, RequestVerificationError>
where
    R: JwksResolver,
    C: TimeSource,
{
    let resource = verifier
        .verify_resource(&req.resource_token, ps_iss)
        .await
        .map_err(RequestVerificationError::ResourceToken)?;
    if resource.claims.aud != ps_iss {
        return Err(RequestVerificationError::ResourceTokenWrongAudience {
            expected: ps_iss.to_string(),
            actual: resource.claims.aud.clone(),
        });
    }

    let agent = verifier
        .verify_agent(agent_token)
        .await
        .map_err(RequestVerificationError::AgentToken)?;
    if parent_agent_of(agent_token)
        .map_err(RequestVerificationError::ParentAgentDecode)?
        .is_some()
    {
        return Err(RequestVerificationError::AgentTokenIsSubagent);
    }

    let subagent = match &req.subagent_token {
        Some(sat) => {
            let verified = verifier
                .verify_agent(sat)
                .await
                .map_err(RequestVerificationError::SubagentToken)?;
            let parent_agent = parent_agent_of(sat)
                .map_err(RequestVerificationError::ParentAgentDecode)?
                .ok_or(RequestVerificationError::SubagentTokenIsItselfSubagent)?;
            if parent_agent != agent.claims.sub {
                return Err(RequestVerificationError::SubagentParentMismatch {
                    parent_agent,
                    signing_agent: agent.claims.sub.clone(),
                });
            }
            if resource.claims.agent != verified.claims.sub {
                return Err(RequestVerificationError::ResourceTokenSubagentIdentifierMismatch);
            }
            Some(verified)
        }
        None => None,
    };

    let binding = match &subagent {
        Some(sub) => {
            if resource.claims.agent_jkt != sub.jkt {
                return Err(RequestVerificationError::ResourceTokenSubagentKeyMismatch);
            }
            BindingAgent {
                agent: sub.claims.sub.clone(),
                agent_jkt: sub.jkt.clone(),
            }
        }
        None => {
            if resource.claims.agent_jkt != agent.jkt {
                return Err(RequestVerificationError::ResourceTokenAgentKeyMismatch);
            }
            BindingAgent {
                agent: agent.claims.sub.clone(),
                agent_jkt: agent.jkt.clone(),
            }
        }
    };

    let upstream = match &req.upstream_token {
        Some(ut) => Some(verify_upstream(verifier, &agent.claims, ut).await?),
        None => None,
    };

    Ok(VerifiedRequest {
        resource_claims: resource.claims,
        agent_claims: agent.claims,
        subagent_claims: subagent.map(|s| s.claims),
        upstream,
        binding,
        justification: req.justification.clone(),
    })
}

/// "Upstream Token Verification" (#upstream-token-verification): the
/// upstream token's `aud` must equal the `iss` of the intermediary's own
/// agent token, binding the delegated grant to the specific hop that
/// presented it.
async fn verify_upstream<R, C>(
    verifier: &TokenVerifier<R, C>,
    intermediary_agent: &AgentClaims,
    upstream_token: &str,
) -> Result<VerifiedAuth, RequestVerificationError>
where
    R: JwksResolver,
    C: TimeSource,
{
    let upstream = verifier
        .verify_auth(upstream_token, &intermediary_agent.iss)
        .await
        .map_err(RequestVerificationError::UpstreamToken)?;

    if upstream.claims.aud != intermediary_agent.iss {
        return Err(RequestVerificationError::UpstreamAudienceBindingMismatch {
            upstream_aud: upstream.claims.aud.clone(),
            agent_token_iss: intermediary_agent.iss.clone(),
        });
    }

    Ok(upstream)
}

#[cfg(test)]
mod tests;
