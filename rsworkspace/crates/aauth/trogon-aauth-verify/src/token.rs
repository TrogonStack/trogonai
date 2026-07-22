//! JWT token verification for the AAuth typ values.

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet},
};
use serde::Deserialize;
use trogon_identity_types::aauth::{AgentClaims, AuthClaims, ResourceClaims, TYP_AGENT, TYP_AUTH, TYP_RESOURCE};

use crate::jwks::{JwksError, JwksResolver};
use crate::time_source::TimeSource;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("missing or malformed JWT header")]
    BadHeader,
    #[error("expected typ={expected}, got {actual:?}")]
    WrongTyp {
        expected: &'static str,
        actual: Option<String>,
    },
    #[error("unsupported algorithm: {0:?}")]
    UnsupportedAlg(Algorithm),
    #[error("jwks: {0}")]
    Jwks(#[from] JwksError),
    /// Wraps the typed `jsonwebtoken` decode/validate error so the source
    /// chain (and the specific variant from upstream) survives past this
    /// boundary instead of being flattened to a String.
    #[error("token signature invalid: {0}")]
    Signature(#[source] jsonwebtoken::errors::Error),
    #[error("no compatible JWK found for token")]
    NoCompatibleJwk,
    #[error("token expired")]
    Expired,
    #[error("token not yet valid")]
    NotYetValid,
    #[error("audience mismatch: expected {expected}, got {actual:?}")]
    AudienceMismatch { expected: String, actual: Option<String> },
    #[error("missing required claim: {0}")]
    MissingClaim(&'static str),
    #[error("invalid claim: {0}")]
    InvalidClaim(&'static str),
}

#[derive(Debug, Clone)]
pub struct VerifiedAgent {
    pub claims: AgentClaims,
    pub jkt: String,
    pub raw_jwt: String,
}

#[derive(Debug, Clone)]
pub struct VerifiedAuth {
    pub claims: AuthClaims,
    pub raw_jwt: String,
}

/// The request-signing context a resource observed for an inbound request:
/// the agent identifier and JWK thumbprint of the key that actually produced
/// the HTTP Message Signature (or PoP-equivalent). [`TokenVerifier::verify_auth_request_context`]
/// compares an auth token's claims against this to implement "Request-Context
/// Binding" rules 5-9.
#[derive(Debug, Clone)]
pub struct RequestSigningContext {
    /// Agent identifier taken from the request's signing context (e.g. the
    /// `sub` of the agent token, or the sub-agent identifier for a
    /// parent-mediated request). Compared against the auth token's `agent`
    /// claim (rule 6).
    pub agent: String,
    /// RFC 7638 JWK Thumbprint of the key that signed the HTTP request.
    /// Compared against the auth token's `cnf.jwk` (rule 7).
    pub signing_jkt: String,
}

/// Typed error for each "Request-Context Binding" rule (#auth-token-verification,
/// rules 5-9). Distinct from [`TokenError`] because these checks run only
/// after JWT Trust Verification (rules 1-4, `TokenVerifier::verify_auth`) has
/// already succeeded and operate against request-time context the resource
/// supplies, not the token alone.
#[derive(Debug, thiserror::Error)]
pub enum RequestContextError {
    /// Rule 5: `aud` must match the resource's own identifier. Distinct from
    /// [`TokenError::AudienceMismatch`], which is enforced during JWT decode;
    /// this variant exists for callers that verify context bindings as a
    /// separate step against an already-decoded [`VerifiedAuth`].
    #[error("aud does not match resource identifier: expected {expected}, got {actual}")]
    AudienceMismatch { expected: String, actual: String },
    /// Rule 6: `agent` must match the agent identifier from the request's
    /// signing context.
    #[error("agent claim does not match request signing context: expected {expected}, got {actual}")]
    AgentMismatch { expected: String, actual: String },
    /// Rule 7 (verbatim): `cnf.jwk` is REQUIRED. Absent entirely.
    #[error("cnf.jwk is required and was absent")]
    MissingConfirmationKey,
    /// Rule 7 (verbatim): `cnf.jwk` is missing `kty` or the members required
    /// for that key type. This is a *distinct* rejection from
    /// [`RequestContextError::InvalidKeyMaterial`] -- the draft calls out
    /// "reject the token as structurally incomplete before attempting key
    /// decoding" as separate from "present but not parseable as a supported
    /// public key".
    #[error("cnf.jwk is structurally incomplete: {0}")]
    StructurallyIncompleteKey(#[source] crate::jkt::JktError),
    /// Rule 7 (verbatim): `cnf.jwk` was structurally complete but not
    /// parseable as a supported public key. The source is either a
    /// deserialize failure (malformed JSON shape not caught by the
    /// structural pre-check) or a `jsonwebtoken` decoding-key construction
    /// failure (e.g. an invalid curve point).
    #[error("cnf.jwk is not valid key material")]
    InvalidKeyMaterial(#[source] InvalidKeyMaterialSourceError),
    /// Rule 7: `cnf.jwk` (structurally valid, parseable key material) does
    /// not match the key that signed the HTTP request.
    #[error("cnf.jwk does not match the request signing key")]
    ConfirmationKeyMismatch,
    /// Rule 8: `act.agent` is not a syntactically valid AAuth agent
    /// identifier.
    #[error("act chain is invalid: {0}")]
    InvalidActChain(#[source] crate::delegation::DelegationError),
    /// Rule 9: neither `sub` nor `scope` is present. `AuthClaims::sub` is a
    /// required (non-`Option`) field on the current claim struct, so in
    /// practice only an empty `scope` alongside an empty `sub` string trips
    /// this; the check is expressed explicitly so a future claim-struct
    /// change that makes `sub` optional stays correctly enforced.
    #[error("neither sub nor scope is present")]
    MissingSubOrScope,
}

/// Source of an [`RequestContextError::InvalidKeyMaterial`] failure: either
/// `cnf.jwk` did not deserialize into the `jsonwebtoken` JWK shape, or it
/// deserialized but `jsonwebtoken` rejected the key material itself (e.g. a
/// curve point that doesn't lie on the curve).
#[derive(Debug, thiserror::Error)]
pub enum InvalidKeyMaterialSourceError {
    #[error("cnf.jwk did not deserialize into a supported JWK shape")]
    Deserialize(#[source] serde_json::Error),
    #[error("cnf.jwk failed to produce a decoding key")]
    DecodingKey(#[source] jsonwebtoken::errors::Error),
}

#[derive(Debug, Clone)]
pub struct VerifiedResource {
    pub claims: ResourceClaims,
    pub raw_jwt: String,
}

/// Verifier for AAuth JWTs. Pluggable JWKS resolver + clock; no global state.
pub struct TokenVerifier<R: JwksResolver, C: TimeSource> {
    jwks: R,
    clock: C,
    leeway_secs: u64,
}

impl<R: JwksResolver, C: TimeSource> TokenVerifier<R, C> {
    pub fn new(jwks: R, clock: C) -> Self {
        Self {
            jwks,
            clock,
            leeway_secs: 60,
        }
    }

    #[must_use]
    pub fn with_leeway(mut self, leeway_secs: u64) -> Self {
        self.leeway_secs = leeway_secs;
        self
    }

    /// Verify an `aa-agent+jwt`. Returns the parsed claims and the JWK thumbprint
    /// of the agent's confirmation key.
    pub async fn verify_agent(&self, jwt: &str) -> Result<VerifiedAgent, TokenError> {
        let (header, alg) = parse_typ(jwt, TYP_AGENT)?;
        let claims_raw: serde_json::Value = self
            .decode_with_jwks(jwt, alg, header.kid.as_deref(), iss_of(jwt)?, None)
            .await?;
        let claims: AgentClaims =
            serde_json::from_value(claims_raw.clone()).map_err(|_| TokenError::MissingClaim("agent claims"))?;
        let jkt = crate::jkt::jwk_thumbprint(&claims.cnf.jwk).map_err(|e| {
            TokenError::InvalidClaim(match e {
                crate::jkt::JktError::MissingKty => "cnf.jwk.kty",
                crate::jkt::JktError::MissingField(f) => f,
                crate::jkt::JktError::UnsupportedKty(_) => "cnf.jwk.kty",
                crate::jkt::JktError::Serialize(_) => "cnf.jwk",
            })
        })?;
        self.assert_freshness(claims.iat, claims.exp)?;
        Ok(VerifiedAgent {
            claims,
            jkt,
            raw_jwt: jwt.to_string(),
        })
    }

    /// Verify an `aa-resource+jwt` issued by a resource. `expected_aud` is the PS
    /// URL (3-party) or AS URL (4-party) that should be receiving the exchange.
    pub async fn verify_resource(&self, jwt: &str, expected_aud: &str) -> Result<VerifiedResource, TokenError> {
        let (header, alg) = parse_typ(jwt, TYP_RESOURCE)?;
        let claims_raw: serde_json::Value = self
            .decode_with_jwks(jwt, alg, header.kid.as_deref(), iss_of(jwt)?, Some(expected_aud))
            .await?;
        let claims: ResourceClaims =
            serde_json::from_value(claims_raw).map_err(|_| TokenError::MissingClaim("resource claims"))?;
        self.assert_freshness(claims.iat, claims.exp)?;
        Ok(VerifiedResource {
            claims,
            raw_jwt: jwt.to_string(),
        })
    }

    /// Verify an `aa-auth+jwt`. `expected_aud` is the resource URL or id that this
    /// gateway / resource serves.
    pub async fn verify_auth(&self, jwt: &str, expected_aud: &str) -> Result<VerifiedAuth, TokenError> {
        let (header, alg) = parse_typ(jwt, TYP_AUTH)?;
        let claims_raw: serde_json::Value = self
            .decode_with_jwks(jwt, alg, header.kid.as_deref(), iss_of(jwt)?, Some(expected_aud))
            .await?;
        let claims: AuthClaims =
            serde_json::from_value(claims_raw).map_err(|_| TokenError::MissingClaim("auth claims"))?;
        self.assert_freshness(claims.iat, claims.exp)?;
        Ok(VerifiedAuth {
            claims,
            raw_jwt: jwt.to_string(),
        })
    }

    /// Checks an already-verified auth token's "Request-Context Binding"
    /// rules 5-9 (#auth-token-verification) against the resource's own
    /// identifier and the request's actual signing context.
    ///
    /// Rules 1-4 (JWT Trust Verification: `typ`, `dwk`/signature, freshness,
    /// `iss` shape) are already enforced by [`Self::verify_auth`] before this
    /// runs. This method is a distinct step (not folded into `verify_auth`)
    /// because rules 5-9 need request-time inputs -- the resource's own
    /// identifier and the key that actually signed the inbound request --
    /// that are not available inside token decoding alone.
    ///
    /// `expected_aud` MUST be the same value passed to `verify_auth`; it is
    /// re-checked here (rather than trusted from the prior call) so this
    /// method is safe to call standalone against a [`VerifiedAuth`] obtained
    /// by any means.
    pub fn verify_auth_request_context(
        &self,
        verified: &VerifiedAuth,
        expected_aud: &str,
        ctx: &RequestSigningContext,
    ) -> Result<(), RequestContextError> {
        let claims = &verified.claims;

        // Rule 5.
        if claims.aud != expected_aud {
            return Err(RequestContextError::AudienceMismatch {
                expected: expected_aud.to_string(),
                actual: claims.aud.clone(),
            });
        }

        // Rule 6.
        if claims.agent != ctx.agent {
            return Err(RequestContextError::AgentMismatch {
                expected: ctx.agent.clone(),
                actual: claims.agent.clone(),
            });
        }

        // Rule 7 (verbatim): cnf.jwk is REQUIRED. Structural incompleteness
        // (missing kty, or missing the members required for that key type)
        // is rejected distinctly from key material that is structurally
        // complete but fails to decode as a supported public key.
        let cnf = claims.cnf.as_ref().ok_or(RequestContextError::MissingConfirmationKey)?;
        // `jwk_thumbprint` fails on exactly the shapes rule 7 calls
        // "structurally incomplete": missing `kty`, missing a member the
        // key's `kty` requires, or a `kty` this crate doesn't recognize.
        // None of its error variants indicate key material that parses
        // structurally but is cryptographically invalid -- that case is
        // caught below by `DecodingKey::from_jwk`.
        let jkt = crate::jkt::jwk_thumbprint(&cnf.jwk).map_err(RequestContextError::StructurallyIncompleteKey)?;
        // jwk_thumbprint already validates presence of the type-specific
        // required members (crv/x/y for EC, crv/x for OKP, n/e for RSA); a
        // JWK that reaches this point but still cannot be parsed into a
        // `jsonwebtoken` decoding key is invalid key material, not merely
        // structurally incomplete.
        let parsed_jwk: Jwk = serde_json::from_value(cnf.jwk.clone())
            .map_err(|e| RequestContextError::InvalidKeyMaterial(InvalidKeyMaterialSourceError::Deserialize(e)))?;
        DecodingKey::from_jwk(&parsed_jwk)
            .map_err(|e| RequestContextError::InvalidKeyMaterial(InvalidKeyMaterialSourceError::DecodingKey(e)))?;
        if jkt != ctx.signing_jkt {
            return Err(RequestContextError::ConfirmationKeyMismatch);
        }

        // Rule 8.
        if let Some(act) = &claims.act {
            crate::delegation::verify_act_chain(act).map_err(RequestContextError::InvalidActChain)?;
        }

        // Rule 9.
        if claims.sub.is_empty() && claims.scope.is_empty() {
            return Err(RequestContextError::MissingSubOrScope);
        }

        Ok(())
    }

    fn assert_freshness(&self, iat: i64, exp: i64) -> Result<(), TokenError> {
        let now = self.clock.now();
        let leeway = i64::try_from(self.leeway_secs).unwrap_or(0);
        if now + leeway < iat {
            return Err(TokenError::NotYetValid);
        }
        if now - leeway > exp {
            return Err(TokenError::Expired);
        }
        Ok(())
    }

    async fn decode_with_jwks(
        &self,
        jwt: &str,
        alg: Algorithm,
        kid: Option<&str>,
        iss: String,
        audience: Option<&str>,
    ) -> Result<serde_json::Value, TokenError> {
        let set: JwkSet = self.jwks.resolve(&iss).await?;
        let jwk = pick_jwk(&set, alg, kid).ok_or(TokenError::NoCompatibleJwk)?;
        let key = DecodingKey::from_jwk(jwk).map_err(TokenError::Signature)?;

        let mut validation = Validation::new(alg);
        validation.leeway = self.leeway_secs;
        validation.set_issuer(&[iss.as_str()]);
        match audience {
            Some(aud) => {
                validation.validate_aud = true;
                validation.set_audience(&[aud]);
            }
            None => {
                validation.validate_aud = false;
            }
        }
        // We do our own iat/exp/nbf checks against the supplied clock;
        // jsonwebtoken uses the system clock unconditionally for all three,
        // and leaving validate_nbf on would silently route the nbf check
        // through SystemTime even though the rest of freshness uses the
        // injected TimeSource.
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.required_spec_claims.remove("exp");

        let data = decode::<serde_json::Value>(jwt, &key, &validation).map_err(|e| {
            // jsonwebtoken collapses every validation failure into the same
            // error type. Surface InvalidAudience as AudienceMismatch so
            // operators can tell "wrong aud" from "signature didn't verify"
            // when triaging — both modes need different remediation.
            match e.kind() {
                jsonwebtoken::errors::ErrorKind::InvalidAudience => TokenError::AudienceMismatch {
                    expected: audience.unwrap_or_default().to_string(),
                    actual: None,
                },
                _ => TokenError::Signature(e),
            }
        })?;
        Ok(data.claims)
    }
}

fn iss_of(jwt: &str) -> Result<String, TokenError> {
    let mut parts = jwt.splitn(3, '.');
    let _ = parts.next();
    let payload_b64 = parts.next().ok_or(TokenError::BadHeader)?;
    let payload = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        payload_b64.as_bytes(),
    )
    .map_err(|_| TokenError::BadHeader)?;
    #[derive(Deserialize)]
    struct IssOnly {
        iss: String,
    }
    let v: IssOnly = serde_json::from_slice(&payload).map_err(|_| TokenError::MissingClaim("iss"))?;
    Ok(v.iss)
}

fn parse_typ(jwt: &str, expected: &'static str) -> Result<(jsonwebtoken::Header, Algorithm), TokenError> {
    let header = decode_header(jwt).map_err(|_| TokenError::BadHeader)?;
    let typ = header.typ.as_deref();
    if typ != Some(expected) {
        return Err(TokenError::WrongTyp {
            expected,
            actual: typ.map(str::to_string),
        });
    }
    let alg = header.alg;
    if !matches!(alg, Algorithm::ES256 | Algorithm::ES384 | Algorithm::EdDSA) {
        return Err(TokenError::UnsupportedAlg(alg));
    }
    Ok((header, alg))
}

fn pick_jwk<'a>(set: &'a JwkSet, alg: Algorithm, kid: Option<&str>) -> Option<&'a Jwk> {
    let compat: Vec<&Jwk> = set.keys.iter().filter(|k| jwk_compatible_with_alg(k, alg)).collect();
    if let Some(kid) = kid
        && let Some(j) = compat.iter().copied().find(|j| j.common.key_id.as_deref() == Some(kid))
    {
        return Some(j);
    }
    if compat.len() == 1 {
        return Some(compat[0]);
    }
    None
}

fn jwk_compatible_with_alg(jwk: &Jwk, alg: Algorithm) -> bool {
    match (&jwk.algorithm, alg) {
        (AlgorithmParameters::EllipticCurve(ec), Algorithm::ES256) => ec.curve == EllipticCurve::P256,
        (AlgorithmParameters::EllipticCurve(ec), Algorithm::ES384) => ec.curve == EllipticCurve::P384,
        (AlgorithmParameters::OctetKeyPair(okp), Algorithm::EdDSA) => okp.curve == EllipticCurve::Ed25519,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
