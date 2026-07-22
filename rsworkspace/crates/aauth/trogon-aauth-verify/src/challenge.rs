//! Mint `aa-resource+jwt` challenge tokens (the 401 response body), and
//! verify them from the agent's side after receiving such a challenge
//! (#resource-challenge-verification).

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use trogon_identity_types::aauth::{DWK_RESOURCE, MissionRef, TYP_RESOURCE};

use crate::jwks::JwksResolver;
use crate::time_source::TimeSource;
use crate::token::{TokenError, TokenVerifier, VerifiedResource};

#[derive(Debug, thiserror::Error)]
pub enum ChallengeError {
    /// Wraps the typed `jsonwebtoken` encode error so the source chain is
    /// preserved instead of being flattened to a String.
    #[error("encode: {0}")]
    Encode(#[from] jsonwebtoken::errors::Error),
    #[error("ttl overflowed i64 when added to iat ({iat} + {ttl_secs})")]
    TtlOverflow { iat: i64, ttl_secs: i64 },
}

/// Inputs to mint a resource challenge token.
pub struct ResourceChallenge<'a> {
    pub iss: &'a str,
    pub aud_ps: &'a str,
    pub agent: &'a str,
    pub agent_jkt: &'a str,
    pub scope: &'a str,
    pub ttl_secs: i64,
    pub kid: &'a str,
    pub jti: &'a str,
    pub mission: Option<MissionRef>,
}

/// Mints the resource challenge JWT.
pub struct ChallengeMinter<C: TimeSource> {
    pub signing_key: EncodingKey,
    pub alg: Algorithm,
    pub clock: C,
}

impl<C: TimeSource> ChallengeMinter<C> {
    pub fn new(signing_key: EncodingKey, alg: Algorithm, clock: C) -> Self {
        Self {
            signing_key,
            alg,
            clock,
        }
    }

    pub fn mint(&self, c: &ResourceChallenge<'_>) -> Result<String, ChallengeError> {
        let iat = self.clock.now();
        let exp = iat.checked_add(c.ttl_secs).ok_or(ChallengeError::TtlOverflow {
            iat,
            ttl_secs: c.ttl_secs,
        })?;

        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            aud: &'a str,
            jti: &'a str,
            iat: i64,
            exp: i64,
            dwk: &'a str,
            agent: &'a str,
            agent_jkt: &'a str,
            scope: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            mission: Option<&'a MissionRef>,
        }

        let mut header = Header::new(self.alg);
        header.typ = Some(TYP_RESOURCE.into());
        header.kid = Some(c.kid.into());

        encode(
            &header,
            &Claims {
                iss: c.iss,
                aud: c.aud_ps,
                jti: c.jti,
                iat,
                exp,
                dwk: DWK_RESOURCE,
                agent: c.agent,
                agent_jkt: c.agent_jkt,
                scope: c.scope,
                mission: c.mission.as_ref(),
            },
            &self.signing_key,
        )
        .map_err(ChallengeError::from)
    }
}

/// One-shot helper for callers that don't keep a long-lived minter.
pub fn mint_resource_jwt(
    signing_key: &EncodingKey,
    alg: Algorithm,
    iat: i64,
    c: &ResourceChallenge<'_>,
) -> Result<String, ChallengeError> {
    struct Fixed(i64);
    impl TimeSource for Fixed {
        fn now(&self) -> i64 {
            self.0
        }
    }
    let minter = ChallengeMinter::new(signing_key.clone(), alg, Fixed(iat));
    minter.mint(c)
}

/// The agent-side context needed to check a resource challenge's claims
/// against what the agent itself expects (#resource-challenge-verification,
/// rules 3-6). Rules 1 ("extract the `resource-token` parameter") and 2
/// ("decode and verify the resource token JWT") are the caller's job and
/// [`TokenVerifier::verify_resource`] respectively; rule 7 ("send the
/// resource token to the agent's PS's token endpoint") is a transport step
/// out of scope for this crate.
#[derive(Debug, Clone)]
pub struct ResourceChallengeContext<'a> {
    /// The resource identifier (origin/URL) the agent actually sent its
    /// request to. Compared against the resource token's `iss` (rule 3).
    pub requested_resource: &'a str,
    /// The agent's own identifier. Compared against the resource token's
    /// `agent` (rule 4).
    pub own_agent_identifier: &'a str,
    /// RFC 7638 JWK Thumbprint of the agent's own signing key. Compared
    /// against the resource token's `agent_jkt` (rule 5).
    pub own_signing_jkt: &'a str,
}

/// Typed error for each "Resource Challenge Verification" rule
/// (#resource-challenge-verification). Distinct from [`TokenError`] because
/// these checks run only after JWT Trust Verification (rule 2, performed by
/// [`TokenVerifier::verify_resource`]) has already succeeded, and compare
/// the decoded claims against the agent's own request-time context.
#[derive(Debug, thiserror::Error)]
pub enum ResourceChallengeError {
    /// Rule 2: the resource token itself failed JWT decode/signature/`typ`/
    /// freshness verification.
    #[error("resource token verification failed: {0}")]
    Token(#[from] TokenError),
    /// Rule 3: `iss` does not match the resource the agent sent the request
    /// to.
    #[error("resource token iss does not match the requested resource: expected {expected}, got {actual}")]
    IssuerMismatch { expected: String, actual: String },
    /// Rule 4: `agent` does not match the agent's own identifier.
    #[error("resource token agent does not match own identifier: expected {expected}, got {actual}")]
    AgentMismatch { expected: String, actual: String },
    /// Rule 5: `agent_jkt` does not match the JWK Thumbprint of the agent's
    /// own signing key. Per the draft, for a parent-mediated sub-agent
    /// authorization this instead compares against the `subagent_token`'s
    /// `cnf.jwk` thumbprint; callers in that flow MUST pass the sub-agent's
    /// thumbprint as `own_signing_jkt`.
    #[error("resource token agent_jkt does not match own signing key: expected {expected}, got {actual}")]
    SigningKeyMismatch { expected: String, actual: String },
    /// Rule 6: `exp` is not in the future. Distinct from [`TokenError::Expired`]
    /// only in that it is re-checked here against the caller-supplied clock
    /// as part of the same context-binding step, so this method is safe to
    /// call standalone against an already-verified [`VerifiedResource`].
    #[error("resource token is expired")]
    Expired,
}

/// Verifies an `aa-resource+jwt` challenge from the agent's side
/// (#resource-challenge-verification). Performs rule 2 (JWT decode,
/// signature, `typ`, freshness -- via [`TokenVerifier::verify_resource`])
/// followed by rules 3-6 (`iss`, `agent`, `agent_jkt`, `exp` re-check)
/// against the supplied [`ResourceChallengeContext`].
///
/// `expected_aud` is the PS or AS identifier the agent expects to present
/// this resource token to next (the same value [`TokenVerifier::verify_resource`]
/// would otherwise be given).
pub async fn verify_resource_challenge<R: JwksResolver, C: TimeSource>(
    verifier: &TokenVerifier<R, C>,
    jwt: &str,
    expected_aud: &str,
    ctx: &ResourceChallengeContext<'_>,
    now: i64,
) -> Result<VerifiedResource, ResourceChallengeError> {
    let verified = verifier.verify_resource(jwt, expected_aud).await?;
    let claims = &verified.claims;

    // Rule 3.
    if claims.iss != ctx.requested_resource {
        return Err(ResourceChallengeError::IssuerMismatch {
            expected: ctx.requested_resource.to_string(),
            actual: claims.iss.clone(),
        });
    }

    // Rule 4.
    if claims.agent != ctx.own_agent_identifier {
        return Err(ResourceChallengeError::AgentMismatch {
            expected: ctx.own_agent_identifier.to_string(),
            actual: claims.agent.clone(),
        });
    }

    // Rule 5.
    if claims.agent_jkt != ctx.own_signing_jkt {
        return Err(ResourceChallengeError::SigningKeyMismatch {
            expected: ctx.own_signing_jkt.to_string(),
            actual: claims.agent_jkt.clone(),
        });
    }

    // Rule 6.
    if claims.exp <= now {
        return Err(ResourceChallengeError::Expired);
    }

    Ok(verified)
}

#[cfg(test)]
mod tests;
