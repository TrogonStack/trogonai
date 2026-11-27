use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{CredentialId, CredentialProvider, PullSecretId, TenantId};

use crate::record_codec::RecordCodec;
use crate::stored_credential::StoredCredential;
use crate::{CredentialMaterial, CredentialStore, CredentialStoreError, CredentialSummary, KmsProvider};

type RecordKey = (CredentialProvider, CredentialId);
type MappingKey = (CredentialProvider, PullSecretId);

/// Credentials held in process memory, sealed the same way the durable store seals them.
pub struct InMemoryCredentialStore {
    pub(crate) codec: RecordCodec,
    pub(crate) credentials: Mutex<HashMap<RecordKey, StoredCredential>>,
    pull_secrets: Mutex<HashMap<MappingKey, CredentialId>>,
}

impl InMemoryCredentialStore {
    pub fn new(kms: Arc<dyn KmsProvider>) -> Self {
        Self {
            codec: RecordCodec::new(kms),
            credentials: Mutex::new(HashMap::new()),
            pull_secrets: Mutex::new(HashMap::new()),
        }
    }

    /// Whether legacy (no-AAD) records may still be read.
    pub fn with_legacy_reads(mut self, allow: bool) -> Self {
        self.codec.allow_legacy = allow;
        self
    }
}

#[async_trait]
impl CredentialStore for InMemoryCredentialStore {
    async fn put_credential(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        material: &CredentialMaterial,
    ) -> Result<u32, CredentialStoreError> {
        let stored = self.codec.seal(provider, tenant, credential_id, material, 1).await?;
        self.credentials
            .lock()
            .await
            .insert((provider, credential_id.clone()), stored);
        Ok(1)
    }

    async fn get_material(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Result<CredentialMaterial, CredentialStoreError> {
        let guard = self.credentials.lock().await;
        let stored = guard
            .get(&(provider, credential_id.clone()))
            .ok_or(CredentialStoreError::NotFound)?;
        self.codec.open(provider, tenant, credential_id, stored).await
    }

    async fn rotate_credential(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        material: &CredentialMaterial,
    ) -> Result<u32, CredentialStoreError> {
        let version = self
            .credentials
            .lock()
            .await
            .get(&(provider, credential_id.clone()))
            .map_or(1, |stored| stored.version.saturating_add(1));
        let stored = self
            .codec
            .seal(provider, tenant, credential_id, material, version)
            .await?;
        self.credentials
            .lock()
            .await
            .insert((provider, credential_id.clone()), stored);
        Ok(version)
    }

    async fn reencrypt_credential(
        &self,
        provider: CredentialProvider,
        tenant: Option<&TenantId>,
        credential_id: &CredentialId,
    ) -> Result<u32, CredentialStoreError> {
        let mut guard = self.credentials.lock().await;
        let key = (provider, credential_id.clone());
        let stored = guard.get(&key).ok_or(CredentialStoreError::NotFound)?;
        let rewritten = self.codec.reseal(provider, tenant, credential_id, stored).await?;
        let version = rewritten.version;
        guard.insert(key, rewritten);
        Ok(version)
    }

    async fn list_legacy(&self, provider: CredentialProvider) -> Result<Vec<CredentialId>, CredentialStoreError> {
        let guard = self.credentials.lock().await;
        Ok(guard
            .iter()
            .filter(|((owner, _), stored)| *owner == provider && stored.is_legacy())
            .map(|((_, id), _)| id.clone())
            .collect())
    }

    async fn map_pull_secret(
        &self,
        provider: CredentialProvider,
        pull_secret_id: &PullSecretId,
        credential_id: &CredentialId,
    ) -> Result<(), CredentialStoreError> {
        self.pull_secrets
            .lock()
            .await
            .insert((provider, pull_secret_id.clone()), credential_id.clone());
        Ok(())
    }

    async fn resolve_pull_secret(
        &self,
        provider: CredentialProvider,
        pull_secret_id: &PullSecretId,
    ) -> Result<CredentialId, CredentialStoreError> {
        let mapped = self
            .pull_secrets
            .lock()
            .await
            .get(&(provider, pull_secret_id.clone()))
            .cloned();
        Ok(mapped.unwrap_or_else(|| CredentialId::new(pull_secret_id.as_str())))
    }

    async fn list_credentials(
        &self,
        provider: CredentialProvider,
    ) -> Result<Vec<CredentialSummary>, CredentialStoreError> {
        let guard = self.credentials.lock().await;
        Ok(guard
            .iter()
            .filter(|((owner, _), _)| *owner == provider)
            .map(|((_, id), stored)| CredentialSummary {
                credential_id: id.clone(),
                last_rotated: stored.last_rotated,
                version: stored.version,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests;
