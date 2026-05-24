use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::AuthCalloutError;
use crate::jwt::{derive_caller_id, AudienceAccount, ExternalSubject, SpiceDbPrincipal, UserJwtClaims};

#[derive(Debug)]
pub enum ApiKeyError {
    Empty,
    Unknown,
}

impl fmt::Display for ApiKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("API key must not be empty"),
            Self::Unknown => f.write_str("API key not found in registry"),
        }
    }
}

impl std::error::Error for ApiKeyError {}

impl From<ApiKeyError> for AuthCalloutError {
    fn from(e: ApiKeyError) -> Self {
        Self::CredentialVerification(e.to_string())
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
    fn compute(api_key: &ApiKey, hmac_secret: &[u8]) -> Self {
        let mut mac = Hmac::<Sha256>::new_from_slice(hmac_secret)
            .expect("HMAC accepts any key length");
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
    async fn verify(&self, api_key: &str) -> Result<UserJwtClaims, AuthCalloutError>;
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
    async fn verify(&self, api_key: &str) -> Result<UserJwtClaims, AuthCalloutError> {
        let key = ApiKey::new(api_key).map_err(AuthCalloutError::from)?;
        let entry = self
            .registry
            .lookup(&key)
            .ok_or_else(|| AuthCalloutError::CredentialVerification("unknown API key".into()))?;
        let caller_id = derive_caller_id(entry.external_subject.as_str(), &entry.audience)
            .map_err(|e| AuthCalloutError::CredentialVerification(format!("caller_id derivation failed: {e}")))?;
        let nats_permissions = crate::permissions::IssuedPermissions::default_for_caller(&caller_id);
        Ok(UserJwtClaims {
            sub: entry.external_subject.clone(),
            aud: entry.audience.clone(),
            data: entry.spicedb_principal.clone(),
            caller_id,
            nats_permissions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry() -> ApiKeyRegistry {
        ApiKeyRegistry::new(b"test-hmac-secret".to_vec())
    }

    fn sample_entry() -> ApiKeyEntry {
        ApiKeyEntry {
            spicedb_principal: SpiceDbPrincipal::new("user/alice"),
            audience: AudienceAccount::new("nats-acct-1"),
            external_subject: ExternalSubject::new("alice@example.com").unwrap(),
        }
    }

    #[tokio::test]
    async fn verify_unknown_key_returns_credential_error() {
        let registry = Arc::new(make_registry());
        #[allow(deprecated)]
        let verifier = HmacApiKeyVerifier::new(registry);
        #[allow(deprecated)]
        let err = verifier.verify("not-registered").await.unwrap_err();
        assert!(matches!(err, AuthCalloutError::CredentialVerification(_)));
    }

    #[tokio::test]
    async fn verify_known_key_yields_expected_claims() {
        let mut registry = make_registry();
        let key = ApiKey::new("my-secret-key").unwrap();
        let entry = sample_entry();
        registry.register(&key, entry);
        let registry = Arc::new(registry);
        #[allow(deprecated)]
        let verifier = HmacApiKeyVerifier::new(registry);
        #[allow(deprecated)]
        let claims = verifier.verify("my-secret-key").await.unwrap();
        assert_eq!(claims.sub.as_str(), "alice@example.com");
        assert_eq!(claims.aud.as_str(), "nats-acct-1");
        assert_eq!(claims.data.0["spicedb_subject"], "user/alice");
    }

    #[test]
    fn apikey_rejects_empty() {
        let err = ApiKey::new("").unwrap_err();
        assert!(matches!(err, ApiKeyError::Empty));
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn registry_collides_on_double_register() {
        let mut registry = make_registry();
        let key = ApiKey::new("shared-key").unwrap();
        let first = ApiKeyEntry {
            spicedb_principal: SpiceDbPrincipal::new("user/first"),
            audience: AudienceAccount::new("acct-a"),
            external_subject: ExternalSubject::new("first@example.com").unwrap(),
        };
        let second = ApiKeyEntry {
            spicedb_principal: SpiceDbPrincipal::new("user/second"),
            audience: AudienceAccount::new("acct-b"),
            external_subject: ExternalSubject::new("second@example.com").unwrap(),
        };
        registry.register(&key, first);
        registry.register(&key, second);
        let found = registry.lookup(&key).unwrap();
        assert_eq!(found.external_subject.as_str(), "second@example.com");
    }

    #[test]
    fn digest_is_deterministic() {
        let registry = make_registry();
        let key = ApiKey::new("stable-key").unwrap();
        let d1 = ApiKeyDigest::compute(&key, &registry.hmac_secret);
        let d2 = ApiKeyDigest::compute(&key, &registry.hmac_secret);
        assert_eq!(d1, d2);
    }
}
