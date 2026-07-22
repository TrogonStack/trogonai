//! Resource Challenge Verification, per draft "Resource Access and Resource
//! Tokens" -- the agent-side checks run on receiving a `401`/`402` with
//! `AAuth-Requirement: requirement=auth-token; resource-token="..."`.
//!
//! Steps 1-2 (extracting `resource-token` from the header) are handled by
//! [`trogon_identity_types::aauth::Requirement::parse`] already, so this
//! module starts from an already-extracted resource token JWT.
//!
//! Steps 3-6 (`iss`, `agent`, `agent_jkt`, `exp`) need no network access and
//! are exposed as the pure [`verify_resource_challenge_claims`]. Full JWT
//! signature verification (the part of step 2 that needs JWKS discovery) is
//! deferred to [`trogon_aauth_verify::TokenVerifier::verify_resource`] and
//! exposed as the optional [`verify_resource_challenge_with_jwks`] path for
//! callers with a [`JwksResolver`] available.

use trogon_aauth_verify::TokenVerifier;
use trogon_aauth_verify::jwks::JwksResolver;
use trogon_aauth_verify::time_source::TimeSource;
use trogon_identity_types::aauth::ResourceClaims;

/// Errors from Resource Challenge Verification steps 3-6.
#[derive(Debug, thiserror::Error)]
pub enum ChallengeVerifyError {
    /// Step 1: JWT signature verification against the issuer's JWKS failed.
    #[error("resource token signature verification failed: {0}")]
    SignatureInvalid(#[source] trogon_aauth_verify::TokenError),
    /// Step 3: `iss` did not match the resource the agent sent the request to.
    #[error("resource token iss {actual:?} does not match the resource requested {expected:?}")]
    IssuerMismatch { expected: String, actual: String },
    /// Step 4: `agent` did not match the agent's own identifier.
    #[error("resource token agent {actual:?} does not match own agent id {expected:?}")]
    AgentMismatch { expected: String, actual: String },
    /// Step 5: `agent_jkt` did not match the agent's own signing key thumbprint.
    #[error("resource token agent_jkt {actual:?} does not match own jkt {expected:?}")]
    JktMismatch { expected: String, actual: String },
    /// Step 6: `exp` is not in the future relative to the supplied `now`.
    #[error("resource token exp {exp} is not in the future (now={now})")]
    Expired { exp: i64, now: i64 },
}

/// Steps 3-6 of Resource Challenge Verification, run against an
/// already-decoded [`ResourceClaims`] with no I/O.
///
/// `requested_resource` is the resource the agent sent the original request
/// to (step 3). `own_agent_id` and `own_jkt` are the agent's own identifier
/// and key thumbprint (steps 4-5), e.g. from
/// [`crate::signer::AgentSigner::jkt`]. `now` is the caller-supplied current
/// time in unix seconds (step 6), kept as a parameter so this function stays
/// pure and deterministic for tests.
pub fn verify_resource_challenge_claims(
    claims: &ResourceClaims,
    requested_resource: &str,
    own_agent_id: &str,
    own_jkt: &str,
    now: i64,
) -> Result<(), ChallengeVerifyError> {
    if claims.iss != requested_resource {
        return Err(ChallengeVerifyError::IssuerMismatch {
            expected: requested_resource.to_string(),
            actual: claims.iss.clone(),
        });
    }
    if claims.agent != own_agent_id {
        return Err(ChallengeVerifyError::AgentMismatch {
            expected: own_agent_id.to_string(),
            actual: claims.agent.clone(),
        });
    }
    if claims.agent_jkt != own_jkt {
        return Err(ChallengeVerifyError::JktMismatch {
            expected: own_jkt.to_string(),
            actual: claims.agent_jkt.clone(),
        });
    }
    if claims.exp <= now {
        return Err(ChallengeVerifyError::Expired { exp: claims.exp, now });
    }
    Ok(())
}

/// Full path: verifies the resource token JWT's signature via
/// [`TokenVerifier::verify_resource`], then runs
/// [`verify_resource_challenge_claims`] against the resulting claims.
/// `expected_aud` is the agent's PS/AS URL, matching
/// `TokenVerifier::verify_resource`'s own parameter.
pub async fn verify_resource_challenge_with_jwks<R, C>(
    verifier: &TokenVerifier<R, C>,
    resource_token: &str,
    expected_aud: &str,
    requested_resource: &str,
    own_agent_id: &str,
    own_jkt: &str,
    now: i64,
) -> Result<ResourceClaims, ChallengeVerifyError>
where
    R: JwksResolver,
    C: TimeSource,
{
    let verified = verifier
        .verify_resource(resource_token, expected_aud)
        .await
        .map_err(ChallengeVerifyError::SignatureInvalid)?;
    verify_resource_challenge_claims(&verified.claims, requested_resource, own_agent_id, own_jkt, now)?;
    Ok(verified.claims)
}

#[cfg(test)]
mod tests;
