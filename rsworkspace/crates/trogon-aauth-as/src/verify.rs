//! Verify the inbound PS-to-AS token request per "PS-to-AS Token Request"
//! (#as-token-endpoint), "Parent-Mediated Authorization" (#sub-agents), and
//! "Upstream Token Verification" (#upstream-token-verification).
//!
//! HTTP Message Signature verification of the PS's request (the outer
//! `Signature-Input` / `Signature` per (#http-message-signatures-profile))
//! is a transport concern handled by [`crate::http`]; this module verifies
//! the JWTs carried in the request body and the trust/binding relationships
//! between them.

use trogon_aauth_verify::{JwksResolver, TimeSource, TokenVerifier, VerifiedAuth};
use trogon_identity_types::aauth::federation::AsTokenRequest;
use trogon_identity_types::aauth::{AgentClaims, ResourceClaims};

use crate::error::RequestVerificationError;
use crate::request::BindingAgent;
use crate::subagent::parent_agent_of;
use crate::trust::{TrustRegistry, TrustedIssuer};

/// Everything verified out of one [`AsTokenRequest`], before policy runs.
#[derive(Debug)]
pub struct VerifiedRequest {
    pub resource_claims: ResourceClaims,
    pub agent_claims: AgentClaims,
    pub subagent_claims: Option<AgentClaims>,
    pub upstream: Option<VerifiedAuth>,
    pub binding: BindingAgent,
}

/// Verifies the PS-to-AS request body against this AS's own identity and
/// trust registry. `ps_iss` is the PS issuer that signed the outer HTTP
/// request (already authenticated by the transport layer); it is checked
/// against `trust` per "PS-AS Federation".
pub async fn verify_request<R, C>(
    verifier: &TokenVerifier<R, C>,
    trust: &TrustRegistry,
    as_own_iss: &str,
    ps_iss: &str,
    req: &AsTokenRequest,
) -> Result<(VerifiedRequest, TrustedIssuer), RequestVerificationError>
where
    R: JwksResolver,
    C: TimeSource,
{
    let trusted = trust
        .require(ps_iss)
        .map_err(RequestVerificationError::UntrustedPs)?
        .clone();

    let resource = verifier
        .verify_resource(&req.resource_token, as_own_iss)
        .await
        .map_err(RequestVerificationError::ResourceToken)?;
    if resource.claims.aud != as_own_iss {
        return Err(RequestVerificationError::ResourceTokenWrongAudience {
            expected: as_own_iss.to_string(),
            actual: resource.claims.aud.clone(),
        });
    }

    let agent = verifier
        .verify_agent(&req.agent_token)
        .await
        .map_err(RequestVerificationError::AgentToken)?;
    if parent_agent_of(&req.agent_token)
        .map_err(RequestVerificationError::MalformedParentAgentClaim)?
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
                .map_err(RequestVerificationError::MalformedParentAgentClaim)?
                .ok_or(RequestVerificationError::SubagentTokenIsItselfSubagent)?;
            if parent_agent != agent.claims.sub {
                return Err(RequestVerificationError::SubagentParentMismatch {
                    parent_agent,
                    agent_token_sub: agent.claims.sub.clone(),
                });
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
        Some(ut) => Some(verify_upstream(verifier, trust, &agent.claims, ut).await?),
        None => None,
    };

    Ok((
        VerifiedRequest {
            resource_claims: resource.claims,
            agent_claims: agent.claims,
            subagent_claims: subagent.map(|s| s.claims),
            upstream,
            binding,
        },
        trusted,
    ))
}

/// "Upstream Token Verification" (#upstream-token-verification):
/// 1. Full Auth Token Verification on the upstream token (delegated to
///    [`TokenVerifier::verify_auth`], which checks JWT trust + `aud`
///    binding against `intermediary_agent.iss` per step 3).
/// 2. `iss` must be a trusted issuer.
async fn verify_upstream<R, C>(
    verifier: &TokenVerifier<R, C>,
    trust: &TrustRegistry,
    intermediary_agent: &AgentClaims,
    upstream_token: &str,
) -> Result<VerifiedAuth, RequestVerificationError>
where
    R: JwksResolver,
    C: TimeSource,
{
    // Step 3's audience binding rule: the upstream token's `aud` must equal
    // the `iss` of the intermediary's own agent token (the agent token
    // presented via Signature-Key). `verify_auth` needs the expected `aud`
    // up front, so bind against `intermediary_agent.iss` directly, rather
    // than verifying first and comparing after -- this also means a
    // mismatched `aud` surfaces as the same audience-mismatch failure mode
    // `TokenVerifier` already produces for other token types.
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

    trust
        .require(&upstream.claims.iss)
        .map_err(RequestVerificationError::UpstreamUntrustedIssuer)?;

    Ok(upstream)
}

#[cfg(test)]
mod tests;
