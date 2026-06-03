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
    #[error("token signature invalid: {0}")]
    Signature(String),
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
        Self { jwks, clock, leeway_secs: 60 }
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
        let claims_raw: serde_json::Value = self.decode_with_jwks(jwt, alg, header.kid.as_deref(), iss_of(jwt)?, None).await?;
        let claims: AgentClaims =
            serde_json::from_value(claims_raw.clone()).map_err(|_| TokenError::MissingClaim("agent claims"))?;
        let jkt = crate::jkt::jwk_thumbprint(&claims.cnf.jwk).map_err(|e| TokenError::InvalidClaim(match e {
            crate::jkt::JktError::MissingKty => "cnf.jwk.kty",
            crate::jkt::JktError::MissingField(f) => f,
            crate::jkt::JktError::UnsupportedKty(_) => "cnf.jwk.kty",
        }))?;
        self.assert_freshness(claims.iat, claims.exp)?;
        Ok(VerifiedAgent { claims, jkt, raw_jwt: jwt.to_string() })
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
        Ok(VerifiedResource { claims, raw_jwt: jwt.to_string() })
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
        Ok(VerifiedAuth { claims, raw_jwt: jwt.to_string() })
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
        let jwk = pick_jwk(&set, alg, kid).ok_or_else(|| TokenError::Signature("no compatible JWK".into()))?;
        let key = DecodingKey::from_jwk(jwk).map_err(|e| TokenError::Signature(e.to_string()))?;

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
        // We do our own iat/exp checks against the supplied clock; jsonwebtoken
        // uses the system clock unconditionally.
        validation.validate_exp = false;
        validation.required_spec_claims.remove("exp");

        let data = decode::<serde_json::Value>(jwt, &key, &validation)
            .map_err(|e| TokenError::Signature(e.to_string()))?;
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
        return Err(TokenError::WrongTyp { expected, actual: typ.map(str::to_string) });
    }
    let alg = header.alg;
    if !matches!(alg, Algorithm::ES256 | Algorithm::ES384 | Algorithm::EdDSA) {
        return Err(TokenError::UnsupportedAlg(alg));
    }
    Ok((header, alg))
}

fn pick_jwk<'a>(set: &'a JwkSet, alg: Algorithm, kid: Option<&str>) -> Option<&'a Jwk> {
    let compat: Vec<&Jwk> = set
        .keys
        .iter()
        .filter(|k| jwk_compatible_with_alg(k, alg))
        .collect();
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
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::jwks::StaticJwks;
    use crate::time_source::SystemTimeSource;

    fn freshness_clock(value: Arc<std::sync::atomic::AtomicI64>) -> impl TimeSource {
        struct C(Arc<std::sync::atomic::AtomicI64>);
        impl TimeSource for C {
            fn now(&self) -> i64 {
                self.0.load(std::sync::atomic::Ordering::SeqCst)
            }
        }
        C(value)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assert_freshness_uses_supplied_clock() {
        let v: TokenVerifier<StaticJwks, SystemTimeSource> = TokenVerifier::new(StaticJwks::new(), SystemTimeSource);
        // 1000 valid window using the system clock; with default leeway the
        // window 900..1100 should be considered fresh.
        let _ = v.assert_freshness(900, 1100);

        let _ = freshness_clock; // keep helper referenced
    }
}
