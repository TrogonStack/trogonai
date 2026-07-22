//! Auth Token Response Verification, per draft "Auth Token -- Structure, Usage,
//! Verification" rules 1-6.
//!
//! Rule 1 (JWT signature over the issuer's JWKS) is RECOMMENDED, not required,
//! and needs network access, so it is exposed as a separate optional path
//! (`verify_auth_token_response_with_jwks`) that defers to
//! `trogon_aauth_verify::TokenVerifier::verify_auth` for the actual JWT trust
//! chain -- this module does not hand-roll JWT signature verification.
//! Rules 2-6 are pure struct-level checks against an already-decoded
//! [`AuthClaims`] and run with no I/O, so they are exposed standalone as
//! [`verify_auth_claims`] for callers that already trust the claims (e.g.
//! because rule 1 was just run) or that intentionally skip signature
//! verification.
//!
//! Each rule fails with its own [`VerifyResponseError`] variant so callers
//! (and tests) can pinpoint exactly which check tripped, rather than a single
//! generic boolean.

use trogon_aauth_verify::TokenVerifier;
use trogon_aauth_verify::jkt::jwk_thumbprint;
use trogon_aauth_verify::jwks::JwksResolver;
use trogon_aauth_verify::time_source::TimeSource;
use trogon_identity_types::aauth::AuthClaims;

/// Successful verification result, carrying the parts of [`AuthClaims`] a
/// caller commonly needs next (e.g. to call
/// [`crate::signer::AgentSigner::with_auth_token`]) plus the parsed
/// delegation chain per deliverable 4, so callers can inspect `act` without
/// re-decoding the claims themselves.
#[derive(Debug, Clone)]
pub struct VerifiedAuthResponse {
    /// The upstream agent identifier from `act.agent`, when the auth token
    /// carries a delegation chain. Populated regardless of whether the
    /// caller asked rule 6 to check it against an expected value.
    pub upstream_agent: Option<String>,
}

/// Errors from Auth Token Response Verification rules 1-6. Variant names
/// mirror the rule they enforce so a test failure or log line names the rule
/// directly.
#[derive(Debug, thiserror::Error)]
pub enum VerifyResponseError {
    /// Rule 1: JWT signature verification against the issuer's JWKS failed.
    #[error("auth token signature verification failed: {0}")]
    SignatureInvalid(#[source] trogon_aauth_verify::TokenError),
    /// Rule 2: `iss` did not match the resource token's `aud`.
    #[error("auth token iss {actual:?} does not match resource token aud {expected:?}")]
    IssuerMismatch { expected: String, actual: String },
    /// Rule 3: `aud` did not match the resource the agent intends to access.
    #[error("auth token aud {actual:?} does not match intended resource {expected:?}")]
    AudienceMismatch { expected: String, actual: String },
    /// Rule 4: `cnf.jwk` did not match the agent's own signing key.
    #[error("auth token cnf.jwk does not match the agent's own key")]
    ConfirmationKeyMismatch,
    /// Rule 4: `cnf` claim was absent, so key confirmation could not be checked.
    #[error("auth token is missing the cnf claim required to verify key confirmation")]
    ConfirmationClaimMissing,
    /// Rule 4: `cnf.jwk` thumbprint computation failed.
    #[error("could not compute jkt for auth token cnf.jwk: {0}")]
    ConfirmationKeyThumbprint(#[source] trogon_aauth_verify::jkt::JktError),
    /// Rule 5: `agent` did not match the agent's own identifier.
    #[error("auth token agent {actual:?} does not match own agent id {expected:?}")]
    AgentMismatch { expected: String, actual: String },
    /// Rule 6: `act.agent` did not match the caller-supplied expected upstream agent.
    #[error("auth token act.agent {actual:?} does not match expected upstream agent {expected:?}")]
    UpstreamAgentMismatch { expected: String, actual: String },
}

/// Rules 2-6 of Auth Token Response Verification, run against an
/// already-decoded [`AuthClaims`] with no I/O.
///
/// `resource_token_aud` is the `aud` claim from the resource token the agent
/// originally presented to the token endpoint (rule 2). `intended_resource`
/// is the resource URL/id the agent means to access (rule 3). `own_agent_jwk`
/// is the agent's own public JWK, e.g. [`crate::signer::AgentSigner::public_jwk`]
/// (rule 4). `own_agent_id` is the agent's own identifier (rule 5).
/// `expected_upstream_agent`, if supplied, is checked against `act.agent`
/// when `act` is present (rule 6); per the draft, a caller that does not
/// supply one is not treated as a failure, since the SDK cannot know the
/// expected upstream agent on its own.
pub fn verify_auth_claims(
    claims: &AuthClaims,
    resource_token_aud: &str,
    intended_resource: &str,
    own_agent_jwk: &serde_json::Value,
    own_agent_id: &str,
    expected_upstream_agent: Option<&str>,
) -> Result<VerifiedAuthResponse, VerifyResponseError> {
    // Rule 2: iss must match the resource token's aud.
    if claims.iss != resource_token_aud {
        return Err(VerifyResponseError::IssuerMismatch {
            expected: resource_token_aud.to_string(),
            actual: claims.iss.clone(),
        });
    }

    // Rule 3: aud must match the resource the agent intends to access.
    if claims.aud != intended_resource {
        return Err(VerifyResponseError::AudienceMismatch {
            expected: intended_resource.to_string(),
            actual: claims.aud.clone(),
        });
    }

    // Rule 4: cnf.jwk must match the agent's own signing key, compared by
    // thumbprint equality per "Auth Token Structure" -- the wire `cnf` claim
    // is what carries the confirmation key, not the `agent_jkt` field
    // (which mirrors ResourceClaims and is not the confirmation claim, see
    // module docs on AuthClaims).
    let cnf = claims
        .cnf
        .as_ref()
        .ok_or(VerifyResponseError::ConfirmationClaimMissing)?;
    let token_jkt = jwk_thumbprint(&cnf.jwk).map_err(VerifyResponseError::ConfirmationKeyThumbprint)?;
    let own_jkt = jwk_thumbprint(own_agent_jwk).map_err(VerifyResponseError::ConfirmationKeyThumbprint)?;
    if token_jkt != own_jkt {
        return Err(VerifyResponseError::ConfirmationKeyMismatch);
    }

    // Rule 5: agent must match the agent's own identifier.
    if claims.agent != own_agent_id {
        return Err(VerifyResponseError::AgentMismatch {
            expected: own_agent_id.to_string(),
            actual: claims.agent.clone(),
        });
    }

    // Rule 6: if act is present, optionally verify act.agent identifies the
    // expected upstream agent. Always surface it so callers can inspect it
    // themselves even when they didn't ask for the check.
    let upstream_agent = claims.act.as_ref().map(|act| act.agent.clone());
    if let (Some(expected), Some(actual)) = (expected_upstream_agent, upstream_agent.as_deref())
        && expected != actual
    {
        return Err(VerifyResponseError::UpstreamAgentMismatch {
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }

    Ok(VerifiedAuthResponse { upstream_agent })
}

/// Rule 1 plus rules 2-6: verifies the auth token JWT's signature against the
/// issuer's JWKS via [`TokenVerifier::verify_auth`], then runs
/// [`verify_auth_claims`] against the resulting claims.
///
/// This is the RECOMMENDED full path when a [`JwksResolver`] is available.
/// Callers without network access to the issuer's JWKS (or that already
/// trust the claims through another channel) should call
/// [`verify_auth_claims`] directly instead.
pub async fn verify_auth_token_response_with_jwks<R, C>(
    verifier: &TokenVerifier<R, C>,
    auth_jwt: &str,
    resource_token_aud: &str,
    intended_resource: &str,
    own_agent_jwk: &serde_json::Value,
    own_agent_id: &str,
    expected_upstream_agent: Option<&str>,
) -> Result<VerifiedAuthResponse, VerifyResponseError>
where
    R: JwksResolver,
    C: TimeSource,
{
    let verified = verifier
        .verify_auth(auth_jwt, intended_resource)
        .await
        .map_err(VerifyResponseError::SignatureInvalid)?;
    verify_auth_claims(
        &verified.claims,
        resource_token_aud,
        intended_resource,
        own_agent_jwk,
        own_agent_id,
        expected_upstream_agent,
    )
}

#[cfg(test)]
mod tests;
