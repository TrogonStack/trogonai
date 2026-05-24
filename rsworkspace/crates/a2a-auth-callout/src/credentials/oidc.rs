use std::sync::Arc;

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use crate::error::AuthCalloutError;
use crate::jwt::{
    derive_caller_id, spicedb_principal_from_oidc_claims, AudienceAccount, ExternalSubject, UserJwtClaims,
};
use crate::permissions::IssuedPermissions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcIssuerUrl(String);

impl OidcIssuerUrl {
    pub fn parse(url: impl Into<String>) -> Result<Self, AuthCalloutError> {
        let mut s = url.into();
        while s.ends_with('/') {
            s.pop();
        }
        if s.is_empty() {
            return Err(AuthCalloutError::CredentialVerification(
                "OIDC issuer URL is empty".into(),
            ));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcClientId(String);

impl OidcClientId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[allow(dead_code)]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerToken(String);

impl BearerToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

pub enum JwksSource {
    Remote {
        jwks_uri: String,
        http: reqwest::Client,
    },
    Static(Arc<JwkSet>),
}

pub struct JwksOidcVerifier {
    issuer: OidcIssuerUrl,
    expected_id_token_audiences: Vec<String>,
    jwks: JwksSource,
}

impl JwksOidcVerifier {
    pub fn with_static_jwks(
        issuer: OidcIssuerUrl,
        expected_id_token_audiences: Vec<String>,
        jwks: JwkSet,
    ) -> Self {
        Self {
            issuer,
            expected_id_token_audiences,
            jwks: JwksSource::Static(Arc::new(jwks)),
        }
    }

    pub async fn discover(
        issuer: OidcIssuerUrl,
        expected_id_token_audiences: Vec<String>,
    ) -> Result<Self, AuthCalloutError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| AuthCalloutError::CredentialVerification(e.to_string()))?;
        let uri = format!("{}/.well-known/openid-configuration", issuer.as_str());
        let resp = http
            .get(uri)
            .send()
            .await
            .map_err(|e| AuthCalloutError::CredentialVerification(e.to_string()))?;
        let doc: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AuthCalloutError::CredentialVerification(e.to_string()))?;
        let jwks_uri = doc
            .get("jwks_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AuthCalloutError::CredentialVerification("missing jwks_uri in OIDC discovery".into())
            })?;
        Ok(Self {
            issuer,
            expected_id_token_audiences,
            jwks: JwksSource::Remote {
                jwks_uri: jwks_uri.to_owned(),
                http,
            },
        })
    }

    pub(crate) async fn fetch_jwks(&self) -> Result<JwkSet, AuthCalloutError> {
        match &self.jwks {
            JwksSource::Remote { jwks_uri, http } => {
                let resp = http
                    .get(jwks_uri)
                    .send()
                    .await
                    .map_err(|e| AuthCalloutError::CredentialVerification(e.to_string()))?;
                resp.json::<JwkSet>()
                    .await
                    .map_err(|e| AuthCalloutError::CredentialVerification(e.to_string()))
            }
            JwksSource::Static(j) => Ok((**j).clone()),
        }
    }

    fn decoding_key_for_jwk(
        jwk: &jsonwebtoken::jwk::Jwk,
    ) -> Result<DecodingKey, AuthCalloutError> {
        match &jwk.algorithm {
            AlgorithmParameters::RSA(rsa) => {
                DecodingKey::from_rsa_components(&rsa.n, &rsa.e).map_err(|e| {
                    AuthCalloutError::CredentialVerification(format!("invalid RSA JWK components: {e}"))
                })
            }
            _ => Err(AuthCalloutError::CredentialVerification(
                "OIDC JWK must be RSA for this verifier".into(),
            )),
        }
    }

    pub async fn verify_internal(
        &self,
        token: &BearerToken,
        account: &AudienceAccount,
    ) -> Result<UserJwtClaims, AuthCalloutError> {
        if self.expected_id_token_audiences.is_empty() {
            return Err(AuthCalloutError::CredentialVerification(
                "no expected OIDC token audiences configured".into(),
            ));
        }
        let jwks = self.fetch_jwks().await?;
        let header = decode_header(token.as_str()).map_err(|e| {
            AuthCalloutError::CredentialVerification(format!("invalid JWT header: {e}"))
        })?;
        let kid = header
            .kid
            .as_ref()
            .ok_or_else(|| AuthCalloutError::CredentialVerification("JWT header missing kid".into()))?;
        let jwk = jwks.find(kid).ok_or_else(|| {
            AuthCalloutError::CredentialVerification(format!("no JWK for kid {kid}"))
        })?;
        let auds: Vec<&str> = self
            .expected_id_token_audiences
            .iter()
            .map(String::as_str)
            .collect();
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&auds);
        let decoding_key = Self::decoding_key_for_jwk(jwk)?;
        let token_data =
            decode::<serde_json::Value>(token.as_str(), &decoding_key, &validation).map_err(|e| {
                AuthCalloutError::CredentialVerification(format!("OIDC token validation failed: {e}"))
            })?;
        let sub_str = token_data
            .claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthCalloutError::CredentialVerification("OIDC token missing sub".into()))?;
        let sub = ExternalSubject::new(sub_str).map_err(|e| {
            AuthCalloutError::CredentialVerification(format!("invalid external subject in OIDC token: {e}"))
        })?;
        let data = spicedb_principal_from_oidc_claims(&token_data.claims);
        let caller_id = derive_caller_id(sub_str, account).map_err(|e| {
            AuthCalloutError::CredentialVerification(format!("caller_id derivation failed: {e}"))
        })?;
        let nats_permissions = IssuedPermissions::default_for_caller(&caller_id);
        Ok(UserJwtClaims {
            kid: crate::signing_key_source::unminted_placeholder(),
            sub,
            aud: account.clone(),
            data,
            caller_id,
            nats_permissions,
        })
    }
}

#[async_trait::async_trait]
pub trait OidcVerifier: Send + Sync + 'static {
    async fn verify(
        &self,
        token: &BearerToken,
        account: &AudienceAccount,
    ) -> Result<UserJwtClaims, AuthCalloutError>;
}

#[async_trait::async_trait]
impl OidcVerifier for JwksOidcVerifier {
    async fn verify(
        &self,
        token: &BearerToken,
        account: &AudienceAccount,
    ) -> Result<UserJwtClaims, AuthCalloutError> {
        self.verify_internal(token, account).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::SigningKey;
    use crate::signing_key_source::{KeyVersion, SigningKeyHandle};
    use std::time::Duration;

    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use jsonwebtoken::jwk::{
        AlgorithmParameters, CommonParameters, Jwk, KeyOperations, PublicKeyUse, RSAKeyParameters, RSAKeyType,
    };
    use serde::Serialize;
    use rand::rngs::OsRng;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;

    fn b64url_uint_be(bytes: &[u8]) -> String {
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len().saturating_sub(1));
        let trimmed = if start >= bytes.len() { &bytes[bytes.len().saturating_sub(1)..] } else { &bytes[start..] };
        URL_SAFE_NO_PAD.encode(trimmed)
    }

    fn test_jwks_and_encoding_key(rng: &mut OsRng) -> (JwkSet, jsonwebtoken::EncodingKey) {
        let key = RsaPrivateKey::new(rng, 2048).expect("rsa key");
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(
            key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
                .expect("pem")
                .as_bytes(),
        )
        .expect("encoding key");
        let public = key.to_public_key();
        let n = b64url_uint_be(&public.n().to_bytes_be());
        let e = b64url_uint_be(&public.e().to_bytes_be());
        let jwk = Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_operations: Some(vec![KeyOperations::Sign]),
                key_id: Some("test-kid".into()),
                x509_url: None,
                x509_chain: None,
                x509_sha1_fingerprint: None,
                x509_sha256_fingerprint: None,
                ..Default::default()
            },
            algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
                key_type: RSAKeyType::RSA,
                n,
                e,
            }),
        };
        (JwkSet { keys: vec![jwk] }, encoding_key)
    }

    #[test]
    fn rejects_empty_audience_config() {
        let rng = &mut OsRng;
        let (jwks, _) = test_jwks_and_encoding_key(rng);
        let v = JwksOidcVerifier::with_static_jwks(
            OidcIssuerUrl::parse("https://issuer.example").unwrap(),
            vec![],
            jwks,
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(v.verify_internal(
                &BearerToken::new("x.y.z"),
                &AudienceAccount::new("acct"),
            ))
            .unwrap_err();
        assert!(matches!(err, AuthCalloutError::CredentialVerification(_)));
    }

    #[tokio::test]
    async fn verify_happy_path_rs256() {
        let rng = &mut OsRng;
        let (jwks, enc) = test_jwks_and_encoding_key(rng);
        let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
        let verifier = JwksOidcVerifier::with_static_jwks(
            issuer.clone(),
            vec!["a2a-client".into()],
            jwks,
        );
        #[derive(Serialize)]
        struct IdClaims {
            sub: String,
            iss: String,
            aud: String,
            exp: u64,
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let id = IdClaims {
            sub: "user-42".into(),
            iss: issuer.as_str().to_owned(),
            aud: "a2a-client".into(),
            exp: now + 600,
        };
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("test-kid".into());
        let token = jsonwebtoken::encode(&header, &id, &enc).expect("encode");
        let account = AudienceAccount::new("nats-acct-1");
        let user = verifier
            .verify_internal(&BearerToken::new(token), &account)
            .await
            .unwrap();
        assert_eq!(user.sub.as_str(), "user-42");
        assert_eq!(user.aud.as_str(), "nats-acct-1");
        assert!(!user.caller_id.as_str().contains('.'));
        let handle = SigningKeyHandle::new(
            KeyVersion::new("test").unwrap(),
            SigningKey::from_secret(b"gw-secret-----------------------"),
        );
        let mut user = user;
        user.kid = handle.version().clone();
        let minted = user
            .mint(&handle, std::time::SystemTime::now(), Duration::from_secs(60))
            .unwrap();
        assert!(minted.split('.').count() == 3);
    }

    #[tokio::test]
    async fn verify_fails_bad_signature() {
        let rng = &mut OsRng;
        let (jwks, enc) = test_jwks_and_encoding_key(rng);
        let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
        let verifier = JwksOidcVerifier::with_static_jwks(
            issuer.clone(),
            vec!["a2a-client".into()],
            jwks,
        );
        #[derive(Serialize)]
        struct IdClaims {
            sub: String,
            iss: String,
            aud: String,
            exp: u64,
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let id = IdClaims {
            sub: "user-42".into(),
            iss: issuer.as_str().to_owned(),
            aud: "a2a-client".into(),
            exp: now + 600,
        };
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("test-kid".into());
        let token = jsonwebtoken::encode(&header, &id, &enc).expect("encode");
        let mut parts: Vec<String> = token.split('.').map(String::from).collect();
        {
            let sig = &mut parts[2];
            if let Some(mut c) = sig.pop() {
                c = if c == 'A' { 'B' } else { 'A' };
                sig.push(c);
            }
        }
        let bad = parts.join(".");
        let err = verifier
            .verify_internal(&BearerToken::new(bad), &AudienceAccount::new("acct"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthCalloutError::CredentialVerification(_)));
    }

    #[tokio::test]
    async fn discover_fetches_jwks_via_wiremock() {
        let mock_srv = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/.well-known/openid-configuration"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{"jwks_uri":"{}/jwks"}}"#,
                    mock_srv.uri(),
                ),
                "application/json",
            ))
            .mount(&mock_srv)
            .await;
        let jwk_body = serde_json::json!({"keys":[]});
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/jwks"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(jwk_body.to_string(), "application/json"),
            )
            .mount(&mock_srv)
            .await;
        let issuer = OidcIssuerUrl::parse(mock_srv.uri()).unwrap();
        let v = JwksOidcVerifier::discover(issuer, vec!["aud".into()])
            .await
            .expect("discover");
        let jwks = v.fetch_jwks().await.expect("jwks");
        assert!(jwks.keys.is_empty());
    }
}
