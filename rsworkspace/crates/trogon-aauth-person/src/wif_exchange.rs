//! RFC 8693 Workload Identity Federation token exchange.
//!
//! Composes the four moving parts:
//! 1. Provider lookup (a [`crate::wif::WorkloadIdentityProviderRecord`])
//! 2. Subject token verification (signature + `iss` + `aud` + freshness)
//! 3. CEL claim mapping to derived attributes
//! 4. Service-account-mapping resolution (exactly-one-match)
//!
//! Output is a [`ResolvedWifExchange`] — minting the downstream credential
//! is the caller's job. Keeping the resolver mint-agnostic lets a single
//! pipeline back several minters (mesh JWT, NATS user JWT, AAuth `aa-auth`).

use std::collections::BTreeMap;

use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use trogon_aauth_verify::JwksError;
use trogon_aauth_verify::jwks::JwksResolver;
use trogon_aauth_verify::time_source::TimeSource;

use crate::claim_mapping::{ClaimMappingError, compile_all, evaluate_mappings};
use crate::store::StoreError;
use crate::wif::{KeySource, MappingResolution, WifStore, WorkloadIdentityProviderRecord, resolve_mapping};

pub const SUBJECT_TOKEN_TYPE_JWT: &str = "urn:ietf:params:oauth:token-type:jwt";
pub const SUBJECT_TOKEN_TYPE_ID_TOKEN: &str = "urn:ietf:params:oauth:token-type:id_token";
pub const GRANT_TYPE_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
pub const ISSUED_TOKEN_TYPE_ACCESS_TOKEN: &str = "urn:ietf:params:oauth:token-type:access_token";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WifExchangeRequest {
    pub grant_type: String,
    pub subject_token: String,
    pub subject_token_type: String,
    pub identity_provider_id: String,
    pub service_account_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedWifExchange {
    pub provider_id: String,
    pub mapping_id: String,
    pub service_account_id: String,
    pub permissions: Vec<String>,
    pub attributes: BTreeMap<String, String>,
    pub raw_claims: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum WifExchangeError {
    #[error("unsupported grant_type `{0}`")]
    UnsupportedGrantType(String),
    #[error("unsupported subject_token_type `{0}`")]
    UnsupportedSubjectTokenType(String),
    #[error("unknown identity provider `{0}`")]
    UnknownProvider(String),
    #[error("identity provider `{0}` is disabled")]
    ProviderDisabled(String),
    #[error("requested service account `{0}` does not exist for provider `{1}`")]
    UnknownServiceAccount(String, String),
    #[error("no enabled mapping matches the supplied attributes")]
    NoMappingMatch,
    #[error("multiple mappings matched ({0:?})")]
    AmbiguousMatch(Vec<String>),
    #[error("subject token: {0}")]
    SubjectToken(String),
    #[error("issuer mismatch: token says `{token_iss}`, provider expects `{provider_iss}`")]
    IssuerMismatch { token_iss: String, provider_iss: String },
    #[error("jwks: {0}")]
    Jwks(#[from] JwksError),
    #[error("claim mapping: {0}")]
    ClaimMapping(#[from] ClaimMappingError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

pub struct WifExchangeService<S: WifStore, R: JwksResolver, T: TimeSource> {
    store: S,
    jwks: R,
    clock: T,
    leeway_secs: u64,
}

impl<S: WifStore, R: JwksResolver, T: TimeSource> WifExchangeService<S, R, T> {
    pub fn new(store: S, jwks: R, clock: T) -> Self {
        Self {
            store,
            jwks,
            clock,
            leeway_secs: 60,
        }
    }

    #[must_use]
    pub fn with_leeway_secs(mut self, leeway_secs: u64) -> Self {
        self.leeway_secs = leeway_secs;
        self
    }

    pub async fn resolve(&self, request: &WifExchangeRequest) -> Result<ResolvedWifExchange, WifExchangeError> {
        if request.grant_type != GRANT_TYPE_TOKEN_EXCHANGE {
            return Err(WifExchangeError::UnsupportedGrantType(request.grant_type.clone()));
        }
        if request.subject_token_type != SUBJECT_TOKEN_TYPE_JWT
            && request.subject_token_type != SUBJECT_TOKEN_TYPE_ID_TOKEN
        {
            return Err(WifExchangeError::UnsupportedSubjectTokenType(
                request.subject_token_type.clone(),
            ));
        }

        let provider = self
            .store
            .get_provider(&request.identity_provider_id)
            .await?
            .ok_or_else(|| WifExchangeError::UnknownProvider(request.identity_provider_id.clone()))?;
        if !provider.is_enabled() {
            return Err(WifExchangeError::ProviderDisabled(provider.id.clone()));
        }

        let claims = self.verify_subject_token(&request.subject_token, &provider).await?;

        let compiled = compile_all(&provider.claim_mappings)?;
        let attributes = evaluate_mappings(&compiled, &claims)?;

        let candidates = self.store.list_mappings_for_provider(&provider.id).await?;
        let filtered: Vec<_> = candidates
            .into_iter()
            .filter(|m| m.service_account_id == request.service_account_id)
            .collect();
        if filtered.is_empty() {
            return Err(WifExchangeError::UnknownServiceAccount(
                request.service_account_id.clone(),
                provider.id.clone(),
            ));
        }

        match resolve_mapping(&filtered, &attributes) {
            MappingResolution::NoMatch => Err(WifExchangeError::NoMappingMatch),
            MappingResolution::AmbiguousMatch { matched_ids } => Err(WifExchangeError::AmbiguousMatch(matched_ids)),
            MappingResolution::Matched(record) => Ok(ResolvedWifExchange {
                provider_id: provider.id,
                mapping_id: record.id,
                service_account_id: record.service_account_id,
                permissions: record.permissions,
                attributes,
                raw_claims: claims,
            }),
        }
    }

    async fn verify_subject_token(
        &self,
        subject_token: &str,
        provider: &WorkloadIdentityProviderRecord,
    ) -> Result<serde_json::Value, WifExchangeError> {
        use base64::Engine;

        let header = decode_header(subject_token).map_err(|e| WifExchangeError::SubjectToken(e.to_string()))?;
        let alg = header.alg;

        let mut parts = subject_token.splitn(3, '.');
        let _ = parts.next();
        let payload_b64 = parts
            .next()
            .ok_or_else(|| WifExchangeError::SubjectToken("malformed jwt".into()))?;
        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload_b64.as_bytes())
            .map_err(|e| WifExchangeError::SubjectToken(e.to_string()))?;
        let raw_claims: serde_json::Value =
            serde_json::from_slice(&payload_bytes).map_err(|e| WifExchangeError::SubjectToken(e.to_string()))?;

        let token_iss = raw_claims
            .get("iss")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WifExchangeError::SubjectToken("missing iss".into()))?;
        if token_iss != provider.iss {
            return Err(WifExchangeError::IssuerMismatch {
                token_iss: token_iss.to_string(),
                provider_iss: provider.iss.clone(),
            });
        }

        let jwks = self.resolve_keys(provider).await?;
        let jwk = pick_jwk(&jwks, alg, header.kid.as_deref())
            .ok_or_else(|| WifExchangeError::SubjectToken("no compatible JWK".into()))?;
        let key = DecodingKey::from_jwk(jwk).map_err(|e| WifExchangeError::SubjectToken(e.to_string()))?;

        let mut validation = Validation::new(alg);
        validation.leeway = self.leeway_secs;
        validation.set_issuer(&[provider.iss.as_str()]);
        if provider.audiences.is_empty() {
            validation.validate_aud = false;
        } else {
            let auds: Vec<&str> = provider.audiences.iter().map(String::as_str).collect();
            validation.set_audience(&auds);
            validation.validate_aud = true;
        }
        validation.validate_exp = false;
        validation.required_spec_claims.remove("exp");

        let data = decode::<serde_json::Value>(subject_token, &key, &validation)
            .map_err(|e| WifExchangeError::SubjectToken(e.to_string()))?;

        let now = self.clock.now();
        let leeway = i64::try_from(self.leeway_secs).unwrap_or(0);
        if let Some(exp) = data.claims.get("exp").and_then(|v| v.as_i64())
            && now - leeway > exp
        {
            return Err(WifExchangeError::SubjectToken("token expired".into()));
        }
        if let Some(nbf) = data.claims.get("nbf").and_then(|v| v.as_i64())
            && now + leeway < nbf
        {
            return Err(WifExchangeError::SubjectToken("token not yet valid".into()));
        }

        Ok(data.claims)
    }

    async fn resolve_keys(&self, provider: &WorkloadIdentityProviderRecord) -> Result<JwkSet, WifExchangeError> {
        match &provider.key_source {
            KeySource::InlineJwks { jwks } => Ok(jwks.clone()),
            KeySource::OidcDiscovery { issuer_url } => self.jwks.resolve(issuer_url).await.map_err(WifExchangeError::Jwks),
        }
    }
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
        (AlgorithmParameters::RSA(_), Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wif::{
        ClaimMapping, InMemoryWifStore, KeySource, LifecycleState, MatchPattern, ServiceAccountMappingRecord,
        WorkloadIdentityProviderRecord,
    };
    use jsonwebtoken::{EncodingKey, Header, encode};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};
    use trogon_aauth_verify::StaticJwks;
    use trogon_aauth_verify::time_source::TimeSource;

    #[derive(Clone, Default)]
    struct MockTime(Arc<AtomicI64>);
    impl MockTime {
        fn at(secs: i64) -> Self {
            Self(Arc::new(AtomicI64::new(secs)))
        }
    }
    impl TimeSource for MockTime {
        fn now(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn ed25519_key_pair() -> (EncodingKey, Jwk) {
        use base64::Engine;
        use p256::pkcs8::EncodePrivateKey;
        use rand_core::OsRng;
        let signing = p256::ecdsa::SigningKey::random(&mut OsRng);
        // Switching to ES256 over P-256 because deriving a PEM-encoded EdDSA key
        // in unit tests via jsonwebtoken's EncodingKey is too noisy. ES256 is
        // equally supported and exercises the same code path.
        let verifying = signing.verifying_key();
        let pk = verifying.to_encoded_point(false);
        let x = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pk.x().unwrap());
        let y = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pk.y().unwrap());
        let jwk_json = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": x,
            "y": y,
            "use": "sig",
            "kid": "test-kid",
            "alg": "ES256",
        });
        let jwk: Jwk = serde_json::from_value(jwk_json).unwrap();

        let der = signing.to_pkcs8_der().unwrap();
        let pem_b64 = base64::engine::general_purpose::STANDARD.encode(der.as_bytes());
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{pem_b64}\n-----END PRIVATE KEY-----\n");
        let key = EncodingKey::from_ec_pem(pem.as_bytes()).unwrap();
        (key, jwk)
    }

    fn make_jwt(claims: serde_json::Value, key: &EncodingKey, kid: &str) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(kid.to_string());
        encode(&header, &claims, key).unwrap()
    }

    #[tokio::test]
    async fn happy_path_resolves_to_single_mapping() {
        let (signing, jwk) = ed25519_key_pair();
        let issuer = "https://iss.example".to_string();

        let provider = WorkloadIdentityProviderRecord {
            id: "wif_p".into(),
            iss: issuer.clone(),
            audiences: vec!["openai".into()],
            key_source: KeySource::OidcDiscovery {
                issuer_url: issuer.clone(),
            },
            claim_mappings: vec![ClaimMapping {
                attribute: "openai.subject".into(),
                expression: "assertion.sub".into(),
            }],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        };

        let mut match_attrs = BTreeMap::new();
        match_attrs.insert(
            "openai.subject".into(),
            MatchPattern::Exact { value: "alice".into() },
        );
        let mapping = ServiceAccountMappingRecord {
            id: "m1".into(),
            provider_id: "wif_p".into(),
            match_attributes: match_attrs,
            service_account_id: "svcacct-1".into(),
            permissions: vec!["api.model.read".into()],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        };

        let store = InMemoryWifStore::new();
        store.put_provider(provider).await.unwrap();
        store.put_mapping(mapping).await.unwrap();

        let jwks = StaticJwks::new().with(issuer.clone(), JwkSet { keys: vec![jwk] });
        let clock = MockTime::at(1000);
        let service = WifExchangeService::new(store, jwks, clock);

        let claims = serde_json::json!({
            "iss": issuer,
            "aud": "openai",
            "sub": "alice",
            "exp": 9_999_999_999_i64,
            "iat": 1000,
        });
        let token = make_jwt(claims, &signing, "test-kid");

        let res = service
            .resolve(&WifExchangeRequest {
                grant_type: GRANT_TYPE_TOKEN_EXCHANGE.into(),
                subject_token: token,
                subject_token_type: SUBJECT_TOKEN_TYPE_JWT.into(),
                identity_provider_id: "wif_p".into(),
                service_account_id: "svcacct-1".into(),
            })
            .await
            .expect("resolve");

        assert_eq!(res.mapping_id, "m1");
        assert_eq!(res.service_account_id, "svcacct-1");
        assert_eq!(res.attributes.get("openai.subject"), Some(&"alice".into()));
        assert_eq!(res.permissions, vec!["api.model.read".to_string()]);
    }

    #[tokio::test]
    async fn rejects_unsupported_grant_type() {
        let store = InMemoryWifStore::new();
        let service = WifExchangeService::new(store, StaticJwks::new(), MockTime::at(0));
        let err = service
            .resolve(&WifExchangeRequest {
                grant_type: "password".into(),
                subject_token: "x".into(),
                subject_token_type: SUBJECT_TOKEN_TYPE_JWT.into(),
                identity_provider_id: "p".into(),
                service_account_id: "s".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WifExchangeError::UnsupportedGrantType(_)));
    }

    #[tokio::test]
    async fn rejects_unknown_provider() {
        let store = InMemoryWifStore::new();
        let service = WifExchangeService::new(store, StaticJwks::new(), MockTime::at(0));
        let err = service
            .resolve(&WifExchangeRequest {
                grant_type: GRANT_TYPE_TOKEN_EXCHANGE.into(),
                subject_token: "x".into(),
                subject_token_type: SUBJECT_TOKEN_TYPE_JWT.into(),
                identity_provider_id: "missing".into(),
                service_account_id: "s".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WifExchangeError::UnknownProvider(_)));
    }

    #[tokio::test]
    async fn rejects_when_no_mapping_matches() {
        let (signing, jwk) = ed25519_key_pair();
        let issuer = "https://iss.example".to_string();
        let provider = WorkloadIdentityProviderRecord {
            id: "wif_p".into(),
            iss: issuer.clone(),
            audiences: vec!["openai".into()],
            key_source: KeySource::OidcDiscovery {
                issuer_url: issuer.clone(),
            },
            claim_mappings: vec![ClaimMapping {
                attribute: "openai.subject".into(),
                expression: "assertion.sub".into(),
            }],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        };
        let mut match_attrs = BTreeMap::new();
        match_attrs.insert(
            "openai.subject".into(),
            MatchPattern::Exact {
                value: "different".into(),
            },
        );
        let mapping = ServiceAccountMappingRecord {
            id: "m1".into(),
            provider_id: "wif_p".into(),
            match_attributes: match_attrs,
            service_account_id: "svcacct-1".into(),
            permissions: vec![],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        };
        let store = InMemoryWifStore::new();
        store.put_provider(provider).await.unwrap();
        store.put_mapping(mapping).await.unwrap();

        let jwks = StaticJwks::new().with(issuer.clone(), JwkSet { keys: vec![jwk] });
        let service = WifExchangeService::new(store, jwks, MockTime::at(1000));

        let claims = serde_json::json!({
            "iss": issuer,
            "aud": "openai",
            "sub": "alice",
            "exp": 9_999_999_999_i64,
            "iat": 1000,
        });
        let token = make_jwt(claims, &signing, "test-kid");

        let err = service
            .resolve(&WifExchangeRequest {
                grant_type: GRANT_TYPE_TOKEN_EXCHANGE.into(),
                subject_token: token,
                subject_token_type: SUBJECT_TOKEN_TYPE_JWT.into(),
                identity_provider_id: "wif_p".into(),
                service_account_id: "svcacct-1".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WifExchangeError::NoMappingMatch));
    }

    #[tokio::test]
    async fn rejects_issuer_mismatch() {
        let (signing, jwk) = ed25519_key_pair();
        let provider = WorkloadIdentityProviderRecord {
            id: "wif_p".into(),
            iss: "https://expected.example".into(),
            audiences: vec![],
            key_source: KeySource::InlineJwks {
                jwks: JwkSet { keys: vec![jwk] },
            },
            claim_mappings: vec![],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        };
        let store = InMemoryWifStore::new();
        store.put_provider(provider).await.unwrap();

        let claims = serde_json::json!({
            "iss": "https://attacker.example",
            "sub": "alice",
            "exp": 9_999_999_999_i64,
            "iat": 1000,
        });
        let token = make_jwt(claims, &signing, "test-kid");
        let service = WifExchangeService::new(store, StaticJwks::new(), MockTime::at(1000));

        let err = service
            .resolve(&WifExchangeRequest {
                grant_type: GRANT_TYPE_TOKEN_EXCHANGE.into(),
                subject_token: token,
                subject_token_type: SUBJECT_TOKEN_TYPE_JWT.into(),
                identity_provider_id: "wif_p".into(),
                service_account_id: "svcacct-1".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WifExchangeError::IssuerMismatch { .. }));
    }
}
