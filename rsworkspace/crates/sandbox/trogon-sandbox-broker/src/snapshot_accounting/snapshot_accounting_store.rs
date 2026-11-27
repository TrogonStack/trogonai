use std::collections::HashMap;
use std::sync::Arc;

use async_nats::jetstream::{self, kv};
use tokio::sync::Mutex;
use trogon_sandbox_contract::{SnapshotAccountingRefusal as Refusal, TenantId};
use trogon_sandbox_protocol::subjects::SNAPSHOT_ARTIFACT_KV_BUCKET;
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::backend::Backend;
use super::publication_stream::ensure_publication_stream;
use super::refusal_result::Result;
use super::snapshot_accounting_error::SnapshotAccountingError;
use super::tenant_artifacts::TenantArtifacts;
use crate::constants::{
    SNAPSHOT_ACCOUNTING_BUCKET_MAX_BYTES, SNAPSHOT_ACCOUNTING_MAX_CAS_ATTEMPTS, SNAPSHOT_ACCOUNTING_MAX_KEY_BYTES,
    SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES,
};

pub struct SnapshotAccountingStore {
    pub(super) backend: Backend,
}

impl SnapshotAccountingStore {
    pub fn memory() -> Arc<Self> {
        Arc::new(Self {
            backend: Backend::Memory(Mutex::new(HashMap::new())),
        })
    }

    pub async fn open(js: jetstream::Context) -> std::result::Result<Arc<Self>, SnapshotAccountingError> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: SNAPSHOT_ARTIFACT_KV_BUCKET.to_string(),
                history: 1,
                max_bytes: SNAPSHOT_ACCOUNTING_BUCKET_MAX_BYTES,
                max_value_size: SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES as i32,
                storage: jetstream::stream::StorageType::File,
                ..Default::default()
            })
            .await?;
        let config = &kv.stream.cached_info().config;
        if !(config.max_age.is_zero()
            && config.discard == jetstream::stream::DiscardPolicy::New
            && config.max_messages_per_subject == 1)
        {
            return Err(SnapshotAccountingError::BucketPolicy);
        }
        ensure_publication_stream(&js).await?;
        Ok(Arc::new(Self {
            backend: Backend::Nats {
                kv: Box::new(kv),
                js: Box::new(js),
            },
        }))
    }

    pub(super) async fn read(&self, tenant: &TenantId) -> Result<Option<TenantArtifacts>> {
        match &self.backend {
            Backend::Memory(rows) => Ok(rows.lock().await.get(tenant).cloned()),
            Backend::Nats { kv, .. } => {
                let Some(entry) = kv
                    .entry(tenant_scoped_kv_key(tenant, "artifacts"))
                    .await
                    .map_err(|_| Refusal::Backend)?
                else {
                    return Ok(None);
                };
                if entry.operation != kv::Operation::Put {
                    return Err(Refusal::LegacyUnconfirmed);
                }
                if entry.value.len() > SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES {
                    return Err(Refusal::Capacity);
                }
                let row: TenantArtifacts = serde_json::from_slice(&entry.value).map_err(|_| Refusal::Backend)?;
                if row.tenant != *tenant {
                    return Err(Refusal::OwnershipMismatch);
                }
                row.validate()?;
                Ok(Some(row))
            }
        }
    }

    pub(super) async fn mutate<T>(
        &self,
        tenant: &TenantId,
        mut action: impl FnMut(&mut TenantArtifacts) -> Result<T>,
    ) -> Result<T> {
        if tenant.as_str().is_empty() || tenant.as_str().len() > SNAPSHOT_ACCOUNTING_MAX_KEY_BYTES {
            return Err(Refusal::Capacity);
        }
        match &self.backend {
            Backend::Memory(rows) => {
                let mut rows = rows.lock().await;
                let mut candidate = rows
                    .get(tenant)
                    .cloned()
                    .unwrap_or_else(|| TenantArtifacts::new(tenant.clone()));
                let result = action(&mut candidate)?;
                candidate.encode()?;
                rows.insert(tenant.clone(), candidate);
                Ok(result)
            }
            Backend::Nats { kv, .. } => {
                let key = tenant_scoped_kv_key(tenant, "artifacts");
                for _ in 0..SNAPSHOT_ACCOUNTING_MAX_CAS_ATTEMPTS {
                    let entry = kv.entry(&key).await.map_err(|_| Refusal::Backend)?;
                    let mut candidate = match &entry {
                        Some(entry)
                            if entry.operation == kv::Operation::Put
                                && entry.value.len() <= SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES =>
                        {
                            serde_json::from_slice::<TenantArtifacts>(&entry.value).map_err(|_| Refusal::Backend)?
                        }
                        Some(_) => return Err(Refusal::LegacyUnconfirmed),
                        None => TenantArtifacts::new(tenant.clone()),
                    };
                    candidate.validate()?;
                    if candidate.tenant != *tenant {
                        return Err(Refusal::OwnershipMismatch);
                    }
                    let result = action(&mut candidate)?;
                    let bytes = candidate.encode()?;
                    match entry {
                        Some(entry) => {
                            if bytes.as_slice() == entry.value.as_ref() {
                                return Ok(result);
                            }
                            match kv.update(&key, bytes.into(), entry.revision).await {
                                Ok(_) => return Ok(result),
                                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => {
                                    continue;
                                }
                                Err(_) => return Err(Refusal::Backend),
                            }
                        }
                        None => match kv.create(&key, bytes.into()).await {
                            Ok(_) => return Ok(result),
                            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => {
                                continue;
                            }
                            Err(_) => return Err(Refusal::Backend),
                        },
                    }
                }
                Err(Refusal::Backend)
            }
        }
    }
}
