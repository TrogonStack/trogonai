use std::sync::Arc;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{CredentialId, CredentialProvider, PullSecretId, TenantId};
use trogon_sandbox_protocol::subjects::credential_kv_bucket;

use crate::record_codec::RecordCodec;
use crate::stored_credential::StoredCredential;
use crate::{BackendError, CredentialMaterial, CredentialStore, CredentialStoreError, CredentialSummary, KmsProvider};

const PULL_SECRET_PREFIX: &str = "pull_secret.";

#[derive(Serialize, Deserialize)]
struct PullSecretMapping {
    credential_id: CredentialId,
}

/// Credentials in one NATS KV bucket per provider, sealed at rest.
pub struct NatsKvCredentialStore {
    jetstream: jetstream::Context,
    codec: RecordCodec,
}

impl NatsKvCredentialStore {
    pub fn new(jetstream: jetstream::Context, kms: Arc<dyn KmsProvider>) -> Self {
        Self {
            jetstream,
            codec: RecordCodec::new(kms),
        }
    }

    /// Whether legacy (no-AAD) records may still be read. Turn off once the migration has completed so a
    /// downgraded record cannot be planted.
    pub fn with_legacy_reads(mut self, allow: bool) -> Self {
        self.codec.allow_legacy = allow;
        self
    }

    async fn bucket(&self, provider: CredentialProvider) -> Result<kv::Store, CredentialStoreError> {
        let store = self
            .jetstream
            .create_or_update_key_value(kv::Config {
                bucket: credential_kv_bucket(provider),
                history: 5,
                ..Default::default()
            })
            .await
            .map_err(BackendError::new)?;
        Ok(store)
    }

    async fn put(kv: &kv::Store, key: &str, value: &impl Serialize) -> Result<(), CredentialStoreError> {
        let payload = Bytes::from(serde_json::to_vec(value)?);
        kv.put(key, payload).await.map_err(BackendError::new)?;
        Ok(())
    }

    async fn entry(kv: &kv::Store, id: &CredentialId) -> Result<Option<kv::Entry>, CredentialStoreError> {
        Ok(kv.entry(id.as_str()).await.map_err(BackendError::new)?)
    }

    /// Every credential record of the bucket, skipping the pull secret mappings.
    async fn records(kv: &kv::Store) -> Result<Vec<(CredentialId, StoredCredential)>, CredentialStoreError> {
        let keys: Vec<String> = kv
            .keys()
            .await
            .map_err(BackendError::new)?
            .try_collect()
            .await
            .map_err(BackendError::new)?;
        let mut out = Vec::new();
        for key in keys.into_iter().filter(|key| !key.starts_with(PULL_SECRET_PREFIX)) {
            let id = CredentialId::new(key);
            // A record deleted since the keys were listed is skipped.
            let entry = Self::entry(kv, &id).await?;
            let stored = entry.map(|entry| serde_json::from_slice(&entry.value)).transpose()?;
            out.extend(stored.map(|stored| (id, stored)));
        }
        Ok(out)
    }

    fn mapping_key(pull_secret_id: &PullSecretId) -> String {
        format!("{PULL_SECRET_PREFIX}{pull_secret_id}")
    }
}

#[async_trait]
impl CredentialStore for NatsKvCredentialStore {
    async fn put_credential(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        material: &CredentialMaterial,
    ) -> Result<u32, CredentialStoreError> {
        let kv = self.bucket(provider).await?;
        let stored = self.codec.seal(provider, tenant, credential_id, material, 1).await?;
        Self::put(&kv, credential_id.as_str(), &stored).await?;
        Ok(1)
    }

    async fn get_material(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Result<CredentialMaterial, CredentialStoreError> {
        let kv = self.bucket(provider).await?;
        let entry = Self::entry(&kv, credential_id)
            .await?
            .ok_or(CredentialStoreError::NotFound)?;
        let stored: StoredCredential = serde_json::from_slice(&entry.value)?;
        self.codec.open(provider, tenant, credential_id, &stored).await
    }

    async fn rotate_credential(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        material: &CredentialMaterial,
    ) -> Result<u32, CredentialStoreError> {
        let kv = self.bucket(provider).await?;
        let version = match Self::entry(&kv, credential_id).await? {
            None => 1,
            Some(entry) => serde_json::from_slice::<StoredCredential>(&entry.value)?
                .version
                .saturating_add(1),
        };
        let stored = self
            .codec
            .seal(provider, tenant, credential_id, material, version)
            .await?;
        Self::put(&kv, credential_id.as_str(), &stored).await?;
        Ok(version)
    }

    async fn reencrypt_credential(
        &self,
        provider: CredentialProvider,
        tenant: Option<&TenantId>,
        credential_id: &CredentialId,
    ) -> Result<u32, CredentialStoreError> {
        let kv = self.bucket(provider).await?;
        let entry = Self::entry(&kv, credential_id)
            .await?
            .ok_or(CredentialStoreError::NotFound)?;
        let stored: StoredCredential = serde_json::from_slice(&entry.value)?;
        let rewritten = self.codec.reseal(provider, tenant, credential_id, &stored).await?;
        let payload = Bytes::from(serde_json::to_vec(&rewritten)?);
        // Compare-and-swap so a concurrent rotation is not overwritten.
        kv.update(credential_id.as_str(), payload, entry.revision)
            .await
            .map_err(|_| CredentialStoreError::Conflict)?;
        Ok(rewritten.version)
    }

    async fn list_legacy(&self, provider: CredentialProvider) -> Result<Vec<CredentialId>, CredentialStoreError> {
        let kv = self.bucket(provider).await?;
        Ok(Self::records(&kv)
            .await?
            .into_iter()
            .filter(|(_, stored)| stored.is_legacy())
            .map(|(id, _)| id)
            .collect())
    }

    async fn map_pull_secret(
        &self,
        provider: CredentialProvider,
        pull_secret_id: &PullSecretId,
        credential_id: &CredentialId,
    ) -> Result<(), CredentialStoreError> {
        let kv = self.bucket(provider).await?;
        let mapping = PullSecretMapping {
            credential_id: credential_id.clone(),
        };
        Self::put(&kv, &Self::mapping_key(pull_secret_id), &mapping).await
    }

    async fn resolve_pull_secret(
        &self,
        provider: CredentialProvider,
        pull_secret_id: &PullSecretId,
    ) -> Result<CredentialId, CredentialStoreError> {
        let kv = self.bucket(provider).await?;
        let entry = kv
            .entry(Self::mapping_key(pull_secret_id))
            .await
            .map_err(BackendError::new)?;
        match entry {
            Some(entry) => Ok(serde_json::from_slice::<PullSecretMapping>(&entry.value)?.credential_id),
            None => Ok(CredentialId::new(pull_secret_id.as_str())),
        }
    }

    async fn list_credentials(
        &self,
        provider: CredentialProvider,
    ) -> Result<Vec<CredentialSummary>, CredentialStoreError> {
        let kv = self.bucket(provider).await?;
        Ok(Self::records(&kv)
            .await?
            .into_iter()
            .map(|(credential_id, stored)| CredentialSummary {
                credential_id,
                last_rotated: stored.last_rotated,
                version: stored.version,
            })
            .collect())
    }
}
