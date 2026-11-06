use std::fmt;
use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::constants::OIDC_ALLOWED_ALGORITHMS;
use crate::error::{AuthCalloutError, CredentialError};
use crate::jwt::{
    AudienceAccount, CallerNamespace, ExternalSubject, UserJwtClaims, derive_caller_id,
    spicedb_principal_from_oidc_claims,
};
use crate::permissions::IssuedPermissions;
use crate::{CredentialExpiry, CredentialExpiryError, VerifiedCredential};
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

mod oidc_discovery_error;
mod oidc_issuer_url;

pub use oidc_discovery_error::OidcDiscoveryError;
pub use oidc_issuer_url::{OidcIssuerUrl, OidcIssuerUrlError};

#[derive(serde::Deserialize)]
struct DiscoveryDocumentWire {
    issuer: String,
    jwks_uri: String,
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

#[derive(Clone, PartialEq, Eq)]
pub struct BearerToken(String);

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BearerToken").field(&"<redacted>").finish()
    }
}

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
            .map_err(OidcDiscoveryError::Http)?;
        let uri = format!(
            "{}/.well-known/openid-configuration",
            issuer.as_str().trim_end_matches('/')
        );
        let doc: DiscoveryDocumentWire = oidc_discovery_error::fetch_document(&http, &uri).await?;
        if doc.issuer != issuer.as_str() {
            return Err(OidcDiscoveryError::IssuerMismatch.into());
        }
        if !same_origin(&doc.jwks_uri, issuer.as_str()) {
            return Err(OidcDiscoveryError::JwksLocation.into());
        }
        Ok(Self {
            issuer,
            expected_id_token_audiences,
            jwks: JwksSource::Remote {
                jwks_uri: doc.jwks_uri,
                http,
            },
        })
    }
}

fn same_origin(candidate_url: &str, expected_url: &str) -> bool {
    let Ok(candidate) = reqwest::Url::parse(candidate_url) else {
        return false;
    };
    let Ok(expected) = reqwest::Url::parse(expected_url) else {
        return false;
    };
    candidate.host_str().is_some()
        && candidate.username().is_empty()
        && candidate.password().is_none()
        && candidate.query().is_none()
        && candidate.fragment().is_none()
        && candidate.origin() == expected.origin()
}

/// A JWK may verify an OIDC ID token under `alg` only when the key's own
/// advertised purpose permits verification and its declared algorithm, if any,
/// is the one asserted.
///
/// RFC 7517 makes `use` (section 4.2), `key_ops` (section 4.3), and `alg`
/// (section 4.4) optional, so an absent member stays permissive: an external
/// IdP's JWKS is not ours to constrain beyond what it states. But a publisher
/// that does set them has declared what the key is for, and honoring that stops
/// a key published for encryption, or pinned to one RSA algorithm, from being
/// conscripted into verifying another.
///
/// Mirrors `jwk_compatible_with_alg` in `trogon-aauth-verify`'s token
/// verifier. Kept separate rather than shared because the two crates compile
/// against different major versions of `jsonwebtoken`, so the `Jwk` types are
/// distinct.
fn jwk_permits_verification_with(jwk: &jsonwebtoken::jwk::Jwk, alg: jsonwebtoken::Algorithm) -> bool {
    if let Some(public_key_use) = &jwk.common.public_key_use
        && !matches!(public_key_use, PublicKeyUse::Signature)
    {
        return false;
    }
    if let Some(key_operations) = &jwk.common.key_operations
        && !key_operations.iter().any(|op| matches!(op, KeyOperations::Verify))
    {
        return false;
    }
    let Some(declared) = jwk.common.key_algorithm else {
        return true;
    };
    matches!(
        (declared, alg),
        (KeyAlgorithm::RS256, Algorithm::RS256)
            | (KeyAlgorithm::RS384, Algorithm::RS384)
            | (KeyAlgorithm::RS512, Algorithm::RS512)
            | (KeyAlgorithm::PS256, Algorithm::PS256)
            | (KeyAlgorithm::PS384, Algorithm::PS384)
            | (KeyAlgorithm::PS512, Algorithm::PS512)
    )
}

impl JwksOidcVerifier {
    pub(crate) async fn fetch_jwks(&self) -> Result<JwkSet, AuthCalloutError> {
        match &self.jwks {
            JwksSource::Remote { jwks_uri, http } => oidc_discovery_error::fetch_document(http, jwks_uri)
                .await
                .map_err(AuthCalloutError::from),
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
    ) -> Result<VerifiedCredential, AuthCalloutError> {
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
        if !OIDC_ALLOWED_ALGORITHMS.contains(&header.alg) {
            return Err(CredentialError::UnsupportedTokenAlgorithm { algorithm: header.alg }.into());
        }
        if !jwk_permits_verification_with(jwk, header.alg) {
            return Err(CredentialError::JwkNotPublishedForVerification {
                kid: kid.clone(),
                algorithm: header.alg,
            }
            .into());
        }
        let auds: Vec<&str> = self.expected_id_token_audiences.iter().map(String::as_str).collect();
        // Built from the header only after the allowlist above has vetted it,
        // so the token cannot nominate its own algorithm.
        let mut validation = Validation::new(header.alg);
        validation.leeway = 0;
        validation.validate_nbf = true;
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&auds);
        let decoding_key = Self::decoding_key_for_jwk(jwk)?;
        let token_data = decode::<serde_json::Value>(token.as_str(), &decoding_key, &validation)
            .map_err(|e| CredentialError::InvalidCredentials(format!("OIDC token validation failed: {e}")))?;
        // Header::crit maps explicit null to None, so preserve signed member presence.
        let protected_header = URL_SAFE_NO_PAD
            .decode(token.as_str().split('.').next().unwrap_or_default())
            .map_err(CredentialError::ProtectedHeaderEncoding)?;
        let protected_header: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&protected_header).map_err(CredentialError::ProtectedHeaderJson)?;
        if protected_header.get("crit").is_some() {
            return Err(CredentialError::UnsupportedCriticalHeader.into());
        }
        if token_data
            .claims
            .get("nbf")
            .is_some_and(|value| value.as_i64().is_none())
        {
            return Err(CredentialError::InvalidNotBefore.into());
        }
        let expires_at = token_data
            .claims
            .get("exp")
            .and_then(serde_json::Value::as_i64)
            .ok_or(CredentialError::Validity(CredentialExpiryError::InvalidTimestamp))?;
        let expires_at = CredentialExpiry::from_unix_seconds(expires_at).map_err(CredentialError::Validity)?;
        let sub_str = token_data
            .claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CredentialError::InvalidCredentials("OIDC token missing sub".into()))?;
        let sub = ExternalSubject::new(sub_str)
            .map_err(|e| CredentialError::InvalidCredentials(format!("invalid external subject in OIDC token: {e}")))?;
        let data = spicedb_principal_from_oidc_claims(&token_data.claims);
        let namespace = CallerNamespace::Oidc { issuer: &self.issuer };
        let caller_id = derive_caller_id(namespace, sub_str, account)
            .map_err(|e| CredentialError::InvalidCredentials(format!("caller_id derivation failed: {e}")))?;
        Ok(VerifiedCredential::new(
            UserJwtClaims {
                kid: crate::signing_key_source::unminted_placeholder(),
                sub,
                aud: account.clone(),
                data,
                caller_id,
                principal_kind: namespace.principal_kind(),
                nats_permissions: IssuedPermissions::none(),
            },
            expires_at,
        ))
    }
}

#[async_trait::async_trait]
pub trait OidcVerifier: Send + Sync + 'static {
    async fn verify(
        &self,
        token: &BearerToken,
        account: &AudienceAccount,
    ) -> Result<VerifiedCredential, AuthCalloutError>;
}

#[async_trait::async_trait]
impl OidcVerifier for JwksOidcVerifier {
    async fn verify(
        &self,
        token: &BearerToken,
        account: &AudienceAccount,
    ) -> Result<VerifiedCredential, AuthCalloutError> {
        self.verify_internal(token, account).await
    }
}

#[cfg(test)]
mod tests;
