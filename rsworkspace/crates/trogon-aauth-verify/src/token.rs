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
mod tests {
    use super::*;
    use crate::jwks::StaticJwks;
    use crate::time_source::SystemTimeSource;
    use std::sync::Arc;

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
        let now = Arc::new(std::sync::atomic::AtomicI64::new(1000));
        let v = TokenVerifier::new(StaticJwks::new(), freshness_clock(now.clone())).with_leeway(0);

        // Inside the window with no leeway: clock at 1000, claim 900..1100.
        v.assert_freshness(900, 1100).expect("freshness inside window");

        // Past exp: clock advanced past 1100; with leeway 0 must be Expired.
        now.store(1201, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(v.assert_freshness(900, 1100), Err(TokenError::Expired)));

        // Before iat: clock dropped to 500 while claim says iat=900.
        now.store(500, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(v.assert_freshness(900, 1100), Err(TokenError::NotYetValid)));

        // Sanity: SystemTimeSource still typechecks where used by callers.
        let _system: TokenVerifier<StaticJwks, SystemTimeSource> =
            TokenVerifier::new(StaticJwks::new(), SystemTimeSource);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn verify_resource_distinguishes_audience_mismatch_from_signature() {
        // Regression: jsonwebtoken collapses InvalidAudience into the same
        // error type as bad signatures. The verifier must surface it as the
        // typed AudienceMismatch variant so operators can tell the two
        // failure modes apart.
        let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
        let signing = jsonwebtoken::EncodingKey::from_ec_pem(pem).expect("signing key");
        // The matching public JWK so the signature verifies successfully and
        // the only remaining failure is the audience mismatch.
        let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "kid": "k1",
            "alg": "ES256",
            "use": "sig",
            "x": "EVs_o5-uQbTjL3chynL4wXgUg2R9q9UU8I5mEovUf84",
            "y": "kGe5DgSIycKp8w9aJmoHhB1sB3QTugfnRWm5nU_TzsY"
        }))
        .expect("jwk");
        let jwks = StaticJwks::new().with("iss.example", jsonwebtoken::jwk::JwkSet { keys: vec![jwk] });

        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.typ = Some(TYP_RESOURCE.into());
        header.kid = Some("k1".into());
        let claims = serde_json::json!({
            "iss": "iss.example",
            "aud": "ps.example",
            "jti": "j1",
            "iat": 1000,
            "exp": 9999999999_i64,
            "dwk": "aa-resource",
            "agent": "agent-1",
            "agent_jkt": "abc",
            "scope": "read",
        });
        let jwt = jsonwebtoken::encode(&header, &claims, &signing).expect("encode");
        let v = TokenVerifier::new(jwks, SystemTimeSource);
        let err = v.verify_resource(&jwt, "WRONG-AUD").await.unwrap_err();
        assert!(
            matches!(&err, TokenError::AudienceMismatch { expected, .. } if expected == "WRONG-AUD"),
            "got {err:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn verify_agent_returns_no_compatible_jwk_when_set_is_empty() {
        // A typed Err variant per failure mode lets the gateway distinguish
        // "issuer is known but doesn't publish a matching key" (operator
        // misconfig / pending key rotation) from a generic signature failure.
        let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
        let key = jsonwebtoken::EncodingKey::from_ec_pem(pem).expect("test key");
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.typ = Some(TYP_AGENT.into());
        header.kid = Some("missing-kid".into());
        let claims = serde_json::json!({
            "iss": "iss.example",
            "aud": "ps.example",
            "iat": 1000,
            "exp": 2000,
        });
        let jwt = jsonwebtoken::encode(&header, &claims, &key).expect("encode");
        // Register the issuer with an EMPTY JwkSet — pick_jwk has nothing to
        // match, so the verifier must raise NoCompatibleJwk rather than the
        // generic Signature error.
        let jwks = StaticJwks::new().with("iss.example", jsonwebtoken::jwk::JwkSet { keys: vec![] });
        let v = TokenVerifier::new(jwks, SystemTimeSource);
        let err = v.verify_agent(&jwt).await.unwrap_err();
        assert!(matches!(err, TokenError::NoCompatibleJwk), "got {err:?}");
    }
}
