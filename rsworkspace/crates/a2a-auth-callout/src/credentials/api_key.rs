use std::collections::HashMap;
use std::sync::Arc;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::AuthCalloutError;
use crate::jwt::{AudienceAccount, ExternalSubject, JwtError, SpiceDbPrincipal, UserJwtClaims, derive_caller_id};

#[derive(Debug, thiserror::Error)]
pub enum ApiKeyError {
    #[error("API key must not be empty")]
    Empty,
    #[error("API key not found in registry")]
    Unknown,
    /// `derive_caller_id` rejected the registry entry's external_subject —
    /// preserves the upstream JwtError instead of stringifying it.
    #[error("API key caller_id derivation failed")]
    CallerIdDerivation(#[source] JwtError),
    /// The caller-supplied audience didn't match the registry entry's
    /// audience for this API key.
    #[error("API key audience mismatch: requested={requested:?} registered={registered:?}")]
    AudienceMismatch { requested: String, registered: String },
}

impl From<ApiKeyError> for AuthCalloutError {
    fn from(e: ApiKeyError) -> Self {
        // Variant-to-variant — DenialCategory derives the wire response
        // category from the typed CredentialError tag, not from substring
        // matching on a stringified message.
        match e {
            ApiKeyError::Empty => {
                crate::error::CredentialError::InvalidRequest("API key must not be empty".into()).into()
            }
            ApiKeyError::Unknown => {
                crate::error::CredentialError::InvalidCredentials("API key not found".into()).into()
            }
            ApiKeyError::CallerIdDerivation(je) => {
                crate::error::CredentialError::InvalidCredentials(format!("API key caller_id derivation failed: {je}"))
                    .into()
            }
            ApiKeyError::AudienceMismatch { requested, registered } => crate::error::CredentialError::InvalidRequest(
                format!("API key audience mismatch: requested={requested:?} registered={registered:?}"),
            )
            .into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(key: impl Into<String>) -> Result<Self, ApiKeyError> {
        let s = key.into();
        if s.is_empty() {
            return Err(ApiKeyError::Empty);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApiKeyDigest([u8; 32]);

impl ApiKeyDigest {
    // `Hmac::new_from_slice` only errors for InvalidLength on KeyInit impls
    // that disallow it; `Hmac<Sha256>` accepts any key length, so the result
    // is statically infallible. Documented and intentional.
    #[allow(clippy::expect_used)]
    fn compute(api_key: &ApiKey, hmac_secret: &[u8]) -> Self {
        let mut mac = Hmac::<Sha256>::new_from_slice(hmac_secret).expect("HMAC accepts any key length");
        mac.update(api_key.as_str().as_bytes());
        Self(mac.finalize().into_bytes().into())
    }
}

#[derive(Debug, Clone)]
pub struct ApiKeyEntry {
    pub spicedb_principal: SpiceDbPrincipal,
    pub audience: AudienceAccount,
    pub external_subject: ExternalSubject,
}

pub struct ApiKeyRegistry {
    hmac_secret: Vec<u8>,
    entries: HashMap<ApiKeyDigest, ApiKeyEntry>,
}

impl ApiKeyRegistry {
    pub fn new(hmac_secret: impl Into<Vec<u8>>) -> Self {
        Self {
            hmac_secret: hmac_secret.into(),
            entries: HashMap::new(),
        }
    }

    pub fn register(&mut self, api_key: &ApiKey, entry: ApiKeyEntry) {
        let digest = ApiKeyDigest::compute(api_key, &self.hmac_secret);
        self.entries.insert(digest, entry);
    }

    pub fn lookup(&self, api_key: &ApiKey) -> Option<&ApiKeyEntry> {
        let digest = ApiKeyDigest::compute(api_key, &self.hmac_secret);
        self.entries.get(&digest)
    }
}

#[deprecated(note = "transitional only; remove after OIDC migration")]
#[async_trait::async_trait]
pub trait ApiKeyVerifier: Send + Sync + 'static {
    /// `requested_audience` is the account the connection asked to land in.
    /// API key verifiers reject a key whose registry entry's audience doesn't
    /// match — sibling verifiers (mTLS, OIDC) already drive `aud` from the
    /// connection's resolved audience, so this brings api_key in line.
    async fn verify(
        &self,
        api_key: &str,
        requested_audience: &AudienceAccount,
    ) -> Result<UserJwtClaims, AuthCalloutError>;
}

#[allow(deprecated)]
pub struct HmacApiKeyVerifier {
    registry: Arc<ApiKeyRegistry>,
}

#[allow(deprecated)]
impl HmacApiKeyVerifier {
    pub fn new(registry: Arc<ApiKeyRegistry>) -> Self {
        Self { registry }
    }
}

#[allow(deprecated)]
#[async_trait::async_trait]
impl ApiKeyVerifier for HmacApiKeyVerifier {
    async fn verify(
        &self,
        api_key: &str,
        requested_audience: &AudienceAccount,
    ) -> Result<UserJwtClaims, AuthCalloutError> {
        let key = ApiKey::new(api_key)?;
        let entry = self.registry.lookup(&key).ok_or(ApiKeyError::Unknown)?;
        // mTLS/OIDC drive `aud` from the connection's resolved AudienceAccount,
        // not from registry state. Reject a key whose registry audience
        // doesn't match what the connection is requesting so a leaked key
        // can't mint a JWT for an account the client never asked for.
        if entry.audience != *requested_audience {
            return Err(ApiKeyError::AudienceMismatch {
                requested: requested_audience.as_str().to_owned(),
                registered: entry.audience.as_str().to_owned(),
            }
            .into());
        }
        let caller_id = derive_caller_id(entry.external_subject.as_str(), &entry.audience)
            .map_err(ApiKeyError::CallerIdDerivation)?;
        let nats_permissions = crate::permissions::IssuedPermissions::default_for_caller(&caller_id);
        Ok(UserJwtClaims {
            kid: crate::signing_key_source::unminted_placeholder(),
            sub: entry.external_subject.clone(),
            aud: requested_audience.clone(),
            data: entry.spicedb_principal.clone(),
            caller_id,
            nats_permissions,
        })
    }
}

#[cfg(test)]
mod tests;
