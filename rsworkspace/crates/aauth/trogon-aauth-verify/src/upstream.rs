//! Upstream token verification per draft "Upstream Token Verification"
//! (#upstream-token-verification), used by a PS or AS that receives an
//! `upstream_token` parameter in a call-chaining request (#call-chaining).
//!
//! The draft's numbered rules:
//!
//! 1. Perform Auth Token Verification on the upstream token.
//! 2. Verify `iss` is a trusted issuer (a PS or AS whose auth token the
//!    recipient previously brokered or is authorized to extend).
//! 3. Verify the `aud` in the upstream token equals the `iss` of the
//!    intermediary's agent token (presented in the `Signature-Key` header).
//!    This binding confirms the upstream token was issued to the resource
//!    now making the downstream request.
//! 4. The PS constructs the `act` claim for the downstream auth token:
//!    `act.agent` is set to the intermediary resource's agent identifier, and
//!    if the upstream token contained an `act` claim, it is nested inside as
//!    the new `act.act`.
//! 5. The PS evaluates its mission and governance policy based on the
//!    upstream token's claims and mission context (policy evaluation itself
//!    is out of scope for this crate).
//!
//! Rule 1 (full Auth Token Verification, i.e. `TokenVerifier::verify_auth` +
//! `TokenVerifier::verify_auth_request_context`) is the caller's
//! responsibility before calling [`verify_upstream_token`] -- this function
//! takes the already-verified [`VerifiedAuth`] so callers (trogon-aauth-person,
//! trogon-aauth-as) are not forced to re-implement JWT trust verification.
//! Rule 2 (issuer trust) is exposed as a caller-supplied predicate because
//! "trusted issuer" is a PS/AS-local policy decision this crate cannot make.
//! Rule 5 (mission/governance policy evaluation) is likewise out of scope --
//! this module only produces the inputs (verified claims, constructed `act`)
//! that a policy layer needs.

use trogon_identity_types::aauth::Act;

use crate::token::VerifiedAuth;

/// Errors verifying an upstream token per "Upstream Token Verification".
#[derive(Debug, thiserror::Error)]
pub enum UpstreamTokenError {
    /// Rule 2: the upstream token's `iss` is not trusted by the caller's
    /// policy.
    #[error("upstream token issuer is not trusted: {0}")]
    UntrustedIssuer(String),
    /// Rule 3: the upstream token's `aud` does not equal the intermediary's
    /// agent token `iss`.
    #[error(
        "upstream token aud does not bind to intermediary agent token iss: aud={upstream_aud:?}, agent_token_iss={agent_token_iss:?}"
    )]
    AudienceNotBoundToIntermediary {
        upstream_aud: String,
        agent_token_iss: String,
    },
}

/// Inputs needed to verify rules 2-3 and construct the downstream `act` claim
/// (rule 4). Rule 1 (full Auth Token Verification of `upstream` itself) is
/// assumed already performed by the caller before constructing this.
pub struct UpstreamTokenRequest<'a> {
    /// The upstream auth token, already verified via
    /// `TokenVerifier::verify_auth` + `verify_auth_request_context` (rule 1).
    pub upstream: &'a VerifiedAuth,
    /// The `iss` claim of the intermediary resource's own agent token, as
    /// presented in the `Signature-Key` header of the downstream request
    /// (#call-chaining: "the intermediary signs the downstream token request
    /// with its own key, presenting its own agent token").
    pub intermediary_agent_token_iss: &'a str,
    /// The intermediary resource's own agent identifier, used to build
    /// `act.agent` for the downstream token (rule 4).
    pub intermediary_agent_identifier: &'a str,
}

/// The result of upstream token verification: confirms rules 2-3 passed and
/// supplies the `act` claim the PS/AS should attach to the downstream auth
/// token per rule 4.
#[derive(Debug, Clone)]
pub struct UpstreamVerification {
    /// The `act` claim to attach to the downstream auth token: `agent` is the
    /// intermediary's own identifier, nesting the upstream token's `act` (if
    /// any) as `act.act` -- preserving the complete upstream delegation
    /// chain.
    pub downstream_act: Act,
}

/// Verifies "Upstream Token Verification" rules 2-3 and constructs the `act`
/// claim for rule 4. `is_trusted_issuer` implements rule 2's trust policy
/// (a PS/AS-local decision: "a PS or AS whose auth token the recipient
/// previously brokered or is authorized to extend").
///
/// Rule 1 (full Auth Token Verification on the upstream token) must already
/// have been performed by the caller -- `req.upstream` is expected to be the
/// output of that verification, not a raw unverified token.
pub fn verify_upstream_token(
    req: &UpstreamTokenRequest<'_>,
    is_trusted_issuer: impl FnOnce(&str) -> bool,
) -> Result<UpstreamVerification, UpstreamTokenError> {
    let upstream_claims = &req.upstream.claims;

    // Rule 2.
    if !is_trusted_issuer(&upstream_claims.iss) {
        return Err(UpstreamTokenError::UntrustedIssuer(upstream_claims.iss.clone()));
    }

    // Rule 3.
    if upstream_claims.aud != req.intermediary_agent_token_iss {
        return Err(UpstreamTokenError::AudienceNotBoundToIntermediary {
            upstream_aud: upstream_claims.aud.clone(),
            agent_token_iss: req.intermediary_agent_token_iss.to_string(),
        });
    }

    // Rule 4.
    let downstream_act = Act {
        agent: req.intermediary_agent_identifier.to_string(),
        act: upstream_claims.act.clone().map(Box::new),
    };

    Ok(UpstreamVerification { downstream_act })
}

#[cfg(test)]
mod tests;
