use std::sync::Arc;

use crate::error::AuthCalloutError;
use crate::jwt::{
    AudienceAccount, ExternalSubject, UserJwtClaims, derive_caller_id, spicedb_principal_from_oidc_claims,
};
use crate::permissions::IssuedPermissions;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};

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
    pub fn new(id: impl Into<String>) -> Result<Self, AuthCalloutError> {
        let s = id.into();
        if s.trim().is_empty() {
            return Err(AuthCalloutError::CredentialVerification(
                "OIDC client ID must not be empty".into(),
            ));
        }
        Ok(Self(s))
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
    Remote { jwks_uri: String, http: reqwest::Client },
    Static(Arc<JwkSet>),
}

pub struct JwksOidcVerifier {
    issuer: OidcIssuerUrl,
    expected_id_token_audiences: Vec<String>,
    jwks: JwksSource,
}

impl JwksOidcVerifier {
    pub fn with_static_jwks(issuer: OidcIssuerUrl, expected_id_token_audiences: Vec<String>, jwks: JwkSet) -> Self {
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
        // Bound discovery + JWKS fetches so a misbehaving IdP can't hang the
        // verifier indefinitely. 10s is well above any healthy IdP RTT and
        // still well below typical NATS auth-callout deadlines.
        //
        // Disable redirects so a same-origin `jwks_uri` can't 302 the JWKS
        // fetch off to an attacker-controlled host after origin validation
        // passed at discovery time. The IdP needs to serve JWKS from the
        // same origin directly.
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
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
        // Refuse a discovery doc whose `issuer` claim doesn't match the one
        // we asked for — the doc is fetched off the network and a MITM /
        // misconfigured DNS could redirect us at an attacker's IdP.
        let doc_issuer = doc
            .get("issuer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthCalloutError::CredentialVerification("missing issuer in OIDC discovery".into()))?;
        if doc_issuer.trim_end_matches('/') != issuer.as_str().trim_end_matches('/') {
            return Err(AuthCalloutError::CredentialVerification(format!(
                "OIDC discovery issuer mismatch: configured={:?} discovered={:?}",
                issuer.as_str(),
                doc_issuer
            )));
        }
        let jwks_uri = doc
            .get("jwks_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthCalloutError::CredentialVerification("missing jwks_uri in OIDC discovery".into()))?;
        // Constrain jwks_uri to the issuer's origin so a tampered discovery
        // doc can't point us at attacker-controlled JWKS while tokens still
        // claim the expected `iss`.
        if !same_origin(jwks_uri, issuer.as_str()) {
            return Err(AuthCalloutError::CredentialVerification(format!(
                "OIDC jwks_uri {jwks_uri:?} is outside issuer origin {:?}",
                issuer.as_str()
            )));
        }
        Ok(Self {
            issuer,
            expected_id_token_audiences,
            jwks: JwksSource::Remote {
                jwks_uri: jwks_uri.to_owned(),
                http,
            },
        })
    }
}

/// True if `candidate_url` and `expected_url` share scheme + host + port.
/// Used to keep OIDC `jwks_uri` co-located with the configured issuer so a
/// tampered discovery doc can't redirect JWKS fetches to an attacker origin.
fn same_origin(candidate_url: &str, expected_url: &str) -> bool {
    let cand = match url_origin(candidate_url) {
        Some(o) => o,
        None => return false,
    };
    match url_origin(expected_url) {
        Some(exp) => cand == exp,
        None => false,
    }
}

fn url_origin(s: &str) -> Option<(String, String, Option<String>)> {
    // Lightweight scheme://host[:port] parser — we deliberately don't pull in
    // a full URL crate just for this check.
    let (scheme, rest) = s.split_once("://")?;
    let host_port = rest.split('/').next().unwrap_or("");
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !h.contains('[') || h.ends_with(']') => (h.to_owned(), Some(p.to_owned())),
        _ => (host_port.to_owned(), None),
    };
    if host.is_empty() {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    // Normalize the default ports against the scheme so `https://host` and
    // `https://host:443` collapse to the same origin — otherwise discovery
    // rejects a legitimate jwks_uri whenever the configured issuer omits
    // the default port.
    let port = match (scheme.as_str(), port.as_deref()) {
        ("https", Some("443")) | ("http", Some("80")) => None,
        _ => port,
    };
    Some((scheme, host.to_ascii_lowercase(), port))
}

impl JwksOidcVerifier {
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

    fn decoding_key_for_jwk(jwk: &jsonwebtoken::jwk::Jwk) -> Result<DecodingKey, AuthCalloutError> {
        match &jwk.algorithm {
            AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e)
                .map_err(|e| AuthCalloutError::CredentialVerification(format!("invalid RSA JWK components: {e}"))),
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
        let header = decode_header(token.as_str())
            .map_err(|e| AuthCalloutError::CredentialVerification(format!("invalid JWT header: {e}")))?;
        let kid = header
            .kid
            .as_ref()
            .ok_or_else(|| AuthCalloutError::CredentialVerification("JWT header missing kid".into()))?;
        let jwk = jwks
            .find(kid)
            .ok_or_else(|| AuthCalloutError::CredentialVerification(format!("no JWK for kid {kid}")))?;
        let auds: Vec<&str> = self.expected_id_token_audiences.iter().map(String::as_str).collect();
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&auds);
        let decoding_key = Self::decoding_key_for_jwk(jwk)?;
        let token_data = decode::<serde_json::Value>(token.as_str(), &decoding_key, &validation)
            .map_err(|e| AuthCalloutError::CredentialVerification(format!("OIDC token validation failed: {e}")))?;
        let sub_str = token_data
            .claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthCalloutError::CredentialVerification("OIDC token missing sub".into()))?;
        let sub = ExternalSubject::new(sub_str).map_err(|e| {
            AuthCalloutError::CredentialVerification(format!("invalid external subject in OIDC token: {e}"))
        })?;
        let data = spicedb_principal_from_oidc_claims(&token_data.claims);
        let caller_id = derive_caller_id(sub_str, account)
            .map_err(|e| AuthCalloutError::CredentialVerification(format!("caller_id derivation failed: {e}")))?;
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
    async fn verify(&self, token: &BearerToken, account: &AudienceAccount) -> Result<UserJwtClaims, AuthCalloutError>;
}

#[async_trait::async_trait]
impl OidcVerifier for JwksOidcVerifier {
    async fn verify(&self, token: &BearerToken, account: &AudienceAccount) -> Result<UserJwtClaims, AuthCalloutError> {
        self.verify_internal(token, account).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::SigningKey;
    use crate::signing_key_source::{KeyVersion, SigningKeyHandle};
    use std::time::Duration;

    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtoken::jwk::{
        AlgorithmParameters, CommonParameters, Jwk, KeyOperations, PublicKeyUse, RSAKeyParameters, RSAKeyType,
    };
    use rand::rngs::OsRng;
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::traits::PublicKeyParts;
    use serde::Serialize;

    fn b64url_uint_be(bytes: &[u8]) -> String {
        let start = bytes
            .iter()
            .position(|&b| b != 0)
            .unwrap_or(bytes.len().saturating_sub(1));
        let trimmed = if start >= bytes.len() {
            &bytes[bytes.len().saturating_sub(1)..]
        } else {
            &bytes[start..]
        };
        URL_SAFE_NO_PAD.encode(trimmed)
    }

    fn test_jwks_and_encoding_key(rng: &mut OsRng) -> (JwkSet, jsonwebtoken::EncodingKey) {
        let key = RsaPrivateKey::new(rng, 2048).expect("rsa key");
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(
            key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).expect("pem").as_bytes(),
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
        let v =
            JwksOidcVerifier::with_static_jwks(OidcIssuerUrl::parse("https://issuer.example").unwrap(), vec![], jwks);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(v.verify_internal(&BearerToken::new("x.y.z"), &AudienceAccount::new("acct")))
            .unwrap_err();
        assert!(matches!(err, AuthCalloutError::CredentialVerification(_)));
    }

    #[tokio::test]
    async fn verify_happy_path_rs256() {
        let rng = &mut OsRng;
        let (jwks, enc) = test_jwks_and_encoding_key(rng);
        let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
        let verifier = JwksOidcVerifier::with_static_jwks(issuer.clone(), vec!["a2a-client".into()], jwks);
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
        let issuer = nkeys::KeyPair::new_account();
        let issuer_seed = issuer.seed().expect("issuer seed");
        let subject_kp = nkeys::KeyPair::new_user();
        let handle = SigningKeyHandle::new(
            KeyVersion::new("test").unwrap(),
            SigningKey::from_seed(&issuer_seed).unwrap(),
        );
        let mut user = user;
        user.kid = handle.version().clone();
        let subject = crate::jwt::UserJwtSubject::from_user_nkey(
            crate::wire::NkeyPublic::parse(subject_kp.public_key()).unwrap(),
        );
        let minted = user
            .mint(
                &handle.minting_material(),
                &subject,
                std::time::SystemTime::now(),
                Duration::from_secs(60),
            )
            .unwrap();
        assert!(minted.as_str().split('.').count() == 3);
    }

    #[tokio::test]
    async fn verify_fails_bad_signature() {
        let rng = &mut OsRng;
        let (jwks, enc) = test_jwks_and_encoding_key(rng);
        let issuer = OidcIssuerUrl::parse("https://issuer.example").unwrap();
        let verifier = JwksOidcVerifier::with_static_jwks(issuer.clone(), vec!["a2a-client".into()], jwks);
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
                    r#"{{"issuer":"{}","jwks_uri":"{}/jwks"}}"#,
                    mock_srv.uri(),
                    mock_srv.uri()
                ),
                "application/json",
            ))
            .mount(&mock_srv)
            .await;
        let jwk_body = serde_json::json!({"keys":[]});
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/jwks"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(jwk_body.to_string(), "application/json"))
            .mount(&mock_srv)
            .await;
        let issuer = OidcIssuerUrl::parse(mock_srv.uri()).unwrap();
        let v = JwksOidcVerifier::discover(issuer, vec!["aud".into()])
            .await
            .expect("discover");
        let jwks = v.fetch_jwks().await.expect("jwks");
        assert!(jwks.keys.is_empty());
    }

    #[tokio::test]
    async fn discover_rejects_jwks_uri_outside_issuer_origin() {
        let mock_srv = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/.well-known/openid-configuration"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                format!(
                    r#"{{"issuer":"{}","jwks_uri":"https://attacker.example.com/jwks"}}"#,
                    mock_srv.uri()
                ),
                "application/json",
            ))
            .mount(&mock_srv)
            .await;
        let issuer = OidcIssuerUrl::parse(mock_srv.uri()).unwrap();
        let res = JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await;
        let Err(err) = res else {
            panic!("expected origin mismatch error");
        };
        assert!(err.to_string().contains("outside issuer origin"));
    }

    #[tokio::test]
    async fn discover_rejects_mismatched_issuer_claim() {
        let mock_srv = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/.well-known/openid-configuration"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                r#"{"issuer":"https://other.example.com","jwks_uri":"https://other.example.com/jwks"}"#,
                "application/json",
            ))
            .mount(&mock_srv)
            .await;
        let issuer = OidcIssuerUrl::parse(mock_srv.uri()).unwrap();
        let res = JwksOidcVerifier::discover(issuer, vec!["aud".into()]).await;
        let Err(err) = res else {
            panic!("expected issuer mismatch error");
        };
        assert!(err.to_string().contains("issuer mismatch"));
    }

    #[test]
    fn oidc_client_id_rejects_empty_and_whitespace() {
        assert!(OidcClientId::new("").is_err());
        assert!(OidcClientId::new("   ").is_err());
        assert!(OidcClientId::new("good-client").is_ok());
    }

    #[test]
    fn same_origin_normalizes_default_ports() {
        assert!(super::same_origin(
            "https://idp.example.com/jwks",
            "https://idp.example.com:443"
        ));
        assert!(super::same_origin(
            "https://idp.example.com:443/jwks",
            "https://idp.example.com"
        ));
        assert!(super::same_origin(
            "http://idp.example.com:80/jwks",
            "http://idp.example.com"
        ));
        assert!(!super::same_origin(
            "https://idp.example.com:444/jwks",
            "https://idp.example.com"
        ));
        assert!(!super::same_origin(
            "http://idp.example.com/jwks",
            "https://idp.example.com"
        ));
    }
}
