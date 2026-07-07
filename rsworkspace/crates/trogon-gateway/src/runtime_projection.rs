#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_std::SecretString;

use crate::secret_store::{
    CredentialKind, CredentialOwnerId, CredentialRef, SecretMaterial, SecretStoreError, SecretStoreGet, SourceKind,
};
use crate::source_integration_id::SourceIntegrationId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeIntegrationStatus {
    Active,
    Disabled,
    Archived,
    Deleted,
    Pending,
    Failed,
}

impl RuntimeIntegrationStatus {
    fn is_resolvable(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RuntimeIntegrationKey {
    source: SourceKind,
    integration_id: String,
}

impl RuntimeIntegrationKey {
    pub fn new(source: SourceKind, integration_id: &SourceIntegrationId) -> Self {
        Self {
            source,
            integration_id: integration_id.as_str().to_string(),
        }
    }

    pub fn source(&self) -> SourceKind {
        self.source
    }

    pub fn integration_id(&self) -> &str {
        &self.integration_id
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeIntegrationProjection {
    key: RuntimeIntegrationKey,
    owner_id: CredentialOwnerId,
    status: RuntimeIntegrationStatus,
    version: u64,
    credentials: BTreeMap<CredentialKind, CredentialRef>,
}

impl RuntimeIntegrationProjection {
    pub fn new(
        owner_id: CredentialOwnerId,
        source: SourceKind,
        integration_id: SourceIntegrationId,
        status: RuntimeIntegrationStatus,
        version: u64,
    ) -> Self {
        Self {
            key: RuntimeIntegrationKey::new(source, &integration_id),
            owner_id,
            status,
            version,
            credentials: BTreeMap::new(),
        }
    }

    pub fn with_credential(mut self, kind: CredentialKind, credential: CredentialRef) -> Self {
        self.credentials.insert(kind, credential);
        self
    }

    pub fn key(&self) -> &RuntimeIntegrationKey {
        &self.key
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        &self.owner_id
    }

    pub fn status(&self) -> RuntimeIntegrationStatus {
        self.status
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    fn credential(&self, kind: CredentialKind) -> Option<&CredentialRef> {
        self.credentials.get(&kind)
    }
}

#[derive(Clone, Default)]
pub struct InMemoryRuntimeProjectionRepository {
    projections: Arc<Mutex<BTreeMap<RuntimeIntegrationKey, RuntimeIntegrationProjection>>>,
}

impl InMemoryRuntimeProjectionRepository {
    pub async fn upsert(&self, projection: RuntimeIntegrationProjection) {
        self.projections
            .lock()
            .await
            .insert(projection.key().clone(), projection);
    }

    pub async fn remove(&self, key: &RuntimeIntegrationKey) {
        self.projections.lock().await.remove(key);
    }

    async fn get(&self, key: &RuntimeIntegrationKey) -> Option<RuntimeIntegrationProjection> {
        self.projections.lock().await.get(key).cloned()
    }
}

#[derive(Clone, Default)]
pub struct RuntimeCredentialCache {
    entries: Arc<Mutex<BTreeMap<CredentialRef, SecretMaterial>>>,
}

impl RuntimeCredentialCache {
    async fn get(&self, credential: &CredentialRef) -> Option<SecretMaterial> {
        self.entries.lock().await.get(credential).cloned()
    }

    async fn put(&self, credential: CredentialRef, material: SecretMaterial) {
        self.entries.lock().await.insert(credential, material);
    }

    pub async fn invalidate(&self, credential: &CredentialRef) {
        self.entries.lock().await.remove(credential);
    }

    pub async fn clear(&self) {
        self.entries.lock().await.clear();
    }
}

#[derive(Clone)]
pub struct RuntimeCredentialResolver<S> {
    projections: InMemoryRuntimeProjectionRepository,
    cache: RuntimeCredentialCache,
    store: S,
}

impl<S> RuntimeCredentialResolver<S> {
    pub fn new(projections: InMemoryRuntimeProjectionRepository, store: S) -> Self {
        Self {
            projections,
            cache: RuntimeCredentialCache::default(),
            store,
        }
    }

    pub fn cache(&self) -> &RuntimeCredentialCache {
        &self.cache
    }
}

impl<S> RuntimeCredentialResolver<S>
where
    S: SecretStoreGet<Error = SecretStoreError>,
{
    pub async fn resolve(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
    ) -> Result<SecretMaterial, RuntimeCredentialError> {
        let projection = self
            .projections
            .get(key)
            .await
            .ok_or_else(|| RuntimeCredentialError::IntegrationNotFound { key: key.clone() })?;

        if !projection.status().is_resolvable() {
            return Err(RuntimeCredentialError::IntegrationNotResolvable {
                key: key.clone(),
                status: projection.status(),
            });
        }

        let credential = projection
            .credential(kind)
            .ok_or_else(|| RuntimeCredentialError::CredentialMissing { key: key.clone(), kind })?;

        if let Some(material) = self.cache.get(credential).await {
            return Ok(material);
        }

        let material = self.store.get(credential).await?;
        self.cache.put(credential.clone(), material.clone()).await;
        Ok(material)
    }

    pub async fn resolve_plaintext(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
    ) -> Result<SecretString, RuntimeCredentialError> {
        match self.resolve(key, kind).await? {
            SecretMaterial::Plaintext(value) => Ok(value),
            SecretMaterial::Verifier(_) => Err(RuntimeCredentialError::VerifierOnly { key: key.clone(), kind }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeCredentialError {
    #[error("runtime integration not found: {key}")]
    IntegrationNotFound { key: RuntimeIntegrationKey },
    #[error("runtime integration is not resolvable: {key} is {status:?}")]
    IntegrationNotResolvable {
        key: RuntimeIntegrationKey,
        status: RuntimeIntegrationStatus,
    },
    #[error("runtime credential missing: {key} {kind}")]
    CredentialMissing {
        key: RuntimeIntegrationKey,
        kind: CredentialKind,
    },
    #[error("runtime credential is verifier-only: {key} {kind}")]
    VerifierOnly {
        key: RuntimeIntegrationKey,
        kind: CredentialKind,
    },
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
}

impl RuntimeCredentialError {
    pub fn is_secret_store_error(&self) -> bool {
        matches!(self, Self::SecretStore(_))
    }
}

impl std::fmt::Display for RuntimeIntegrationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.source, self.integration_id)
    }
}

#[cfg(test)]
mod tests {
    use crate::secret_store::{
        CredentialScope, MockOpenBaoSecretStore, SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
    };

    use super::*;

    fn integration_id() -> SourceIntegrationId {
        SourceIntegrationId::new("primary").unwrap()
    }

    fn owner_id() -> CredentialOwnerId {
        CredentialOwnerId::new("tenant-1").unwrap()
    }

    fn key() -> RuntimeIntegrationKey {
        RuntimeIntegrationKey::new(SourceKind::Discord, &integration_id())
    }

    async fn put_bot_token(store: &MockOpenBaoSecretStore, value: &str) -> CredentialRef {
        store
            .put(
                CredentialScope::integration(owner_id(), SourceKind::Discord, integration_id()),
                CredentialKind::BotToken,
                SecretString::new(value).unwrap(),
            )
            .await
            .unwrap()
    }

    fn projection(status: RuntimeIntegrationStatus, credential: CredentialRef) -> RuntimeIntegrationProjection {
        RuntimeIntegrationProjection::new(owner_id(), SourceKind::Discord, integration_id(), status, 1)
            .with_credential(CredentialKind::BotToken, credential)
    }

    #[tokio::test]
    async fn resolves_active_projection_from_secret_store() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, credential))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        let token = resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap();

        assert_eq!(token.as_str(), "Bot token");
    }

    #[tokio::test]
    async fn disabled_projection_fails_closed_without_reading_secret_store() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Disabled, credential))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::IntegrationNotResolvable {
                status: RuntimeIntegrationStatus::Disabled,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn rotation_uses_new_credential_ref_without_reusing_stale_cache() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot old-token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections.clone(), store.clone());

        assert_eq!(
            resolver
                .resolve(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "Bot old-token"
        );

        let rotated = store
            .rotate(&credential, SecretString::new("Bot new-token").unwrap())
            .await
            .unwrap();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, rotated))
            .await;

        assert_eq!(
            resolver
                .resolve(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "Bot new-token"
        );
    }

    #[tokio::test]
    async fn revoked_store_entry_fails_after_cache_invalidation() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store.clone());

        assert!(
            resolver
                .resolve(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_plaintext()
                .is_some()
        );

        store.revoke(&credential).await.unwrap();
        resolver.cache().invalidate(&credential).await;

        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
        ));
    }

    #[tokio::test]
    async fn missing_required_credential_fails_closed() {
        let store = MockOpenBaoSecretStore::default();
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(RuntimeIntegrationProjection::new(
                owner_id(),
                SourceKind::Discord,
                integration_id(),
                RuntimeIntegrationStatus::Active,
                1,
            ))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::CredentialMissing {
                kind: CredentialKind::BotToken,
                ..
            })
        ));
    }
}
