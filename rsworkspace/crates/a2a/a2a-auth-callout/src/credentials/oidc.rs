use std::sync::Arc;

use crate::error::{AuthCalloutError, CredentialError};
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
                CredentialError::InvalidCredentials("OIDC issuer URL is empty".into()),
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
                CredentialError::InvalidCredentials("OIDC client ID must not be empty".into()),
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
            .map_err(|e| CredentialError::InvalidCredentials(e.to_string()))?;
        let uri = format!("{}/.well-known/openid-configuration", issuer.as_str());
        let resp = http
            .get(uri)
            .send()
            .await
            .map_err(|e| CredentialError::InvalidCredentials(e.to_string()))?;
        let doc: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| CredentialError::InvalidCredentials(e.to_string()))?;
        // Refuse a discovery doc whose `issuer` claim doesn't match the one
        // we asked for — the doc is fetched off the network and a MITM /
        // misconfigured DNS could redirect us at an attacker's IdP.
        let doc_issuer = doc
            .get("issuer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CredentialError::InvalidCredentials("missing issuer in OIDC discovery".into()))?;
        if doc_issuer.trim_end_matches('/') != issuer.as_str().trim_end_matches('/') {
            return Err(CredentialError::InvalidCredentials(format!(
                "OIDC discovery issuer mismatch: configured={:?} discovered={:?}",
                issuer.as_str(),
                doc_issuer
            ))
            .into());
        }
        let jwks_uri = doc
            .get("jwks_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CredentialError::InvalidCredentials("missing jwks_uri in OIDC discovery".into()))?;
        // Constrain jwks_uri to the issuer's origin so a tampered discovery
        // doc can't point us at attacker-controlled JWKS while tokens still
        // claim the expected `iss`.
        if !same_origin(jwks_uri, issuer.as_str()) {
            return Err(CredentialError::InvalidCredentials(format!(
                "OIDC jwks_uri {jwks_uri:?} is outside issuer origin {:?}",
                issuer.as_str()
            ))
            .into());
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
                    .map_err(|e| CredentialError::InvalidCredentials(e.to_string()))?;
                resp.json::<JwkSet>()
                    .await
                    .map_err(|e| AuthCalloutError::from(CredentialError::InvalidCredentials(e.to_string())))
            }
            JwksSource::Static(j) => Ok((**j).clone()),
        }
    }

    fn decoding_key_for_jwk(jwk: &jsonwebtoken::jwk::Jwk) -> Result<DecodingKey, AuthCalloutError> {
        match &jwk.algorithm {
            AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e).map_err(|e| {
                AuthCalloutError::from(CredentialError::InvalidCredentials(format!(
                    "invalid RSA JWK components: {e}"
                )))
            }),
            _ => Err(AuthCalloutError::CredentialVerification(
                CredentialError::InvalidCredentials("OIDC JWK must be RSA for this verifier".into()),
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
                CredentialError::InvalidCredentials("no expected OIDC token audiences configured".into()),
            ));
        }
        let jwks = self.fetch_jwks().await?;
        let header = decode_header(token.as_str())
            .map_err(|e| CredentialError::InvalidCredentials(format!("invalid JWT header: {e}")))?;
        let kid = header
            .kid
            .as_ref()
            .ok_or_else(|| CredentialError::InvalidCredentials("JWT header missing kid".into()))?;
        let jwk = jwks
            .find(kid)
            .ok_or_else(|| CredentialError::InvalidCredentials(format!("no JWK for kid {kid}")))?;
        let auds: Vec<&str> = self.expected_id_token_audiences.iter().map(String::as_str).collect();
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&auds);
        let decoding_key = Self::decoding_key_for_jwk(jwk)?;
        let token_data = decode::<serde_json::Value>(token.as_str(), &decoding_key, &validation)
            .map_err(|e| CredentialError::InvalidCredentials(format!("OIDC token validation failed: {e}")))?;
        let sub_str = token_data
            .claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CredentialError::InvalidCredentials("OIDC token missing sub".into()))?;
        let sub = ExternalSubject::new(sub_str)
            .map_err(|e| CredentialError::InvalidCredentials(format!("invalid external subject in OIDC token: {e}")))?;
        let data = spicedb_principal_from_oidc_claims(&token_data.claims);
        let caller_id = derive_caller_id(sub_str, account)
            .map_err(|e| CredentialError::InvalidCredentials(format!("caller_id derivation failed: {e}")))?;
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
mod tests;
