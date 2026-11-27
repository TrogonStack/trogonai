use std::collections::HashMap;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use futures::StreamExt;
use tracing::warn;
use trogon_sandbox_contract::{Digest, SnapshotKvRecord, TenantId};
use trogon_sandbox_protocol::subjects::SNAPSHOT_KV_BUCKET;

use super::catalog::SnapshotCatalog;
use super::snapshot_catalog_error::SnapshotCatalogError;

/// Catalog backed by the snapshot builder's KV bucket.
pub struct NatsKvSnapshotCatalog {
    kv: kv::Store,
}

impl NatsKvSnapshotCatalog {
    /// Opens the bucket with the builder's own configuration, so whichever
    /// process starts first creates it the same way.
    pub async fn open(jetstream: jetstream::Context) -> Result<Self, SnapshotCatalogError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: SNAPSHOT_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl SnapshotCatalog for NatsKvSnapshotCatalog {
    async fn all(&self) -> Result<Vec<SnapshotKvRecord>, SnapshotCatalogError> {
        let mut keys = self.kv.keys().await?;
        let mut out: HashMap<(TenantId, Digest), (SnapshotKvRecord, bool)> = HashMap::new();
        while let Some(key) = keys.next().await {
            let key = key?;
            let Some(entry) = self.kv.entry(&key).await? else {
                continue;
            };
            if entry.operation != kv::Operation::Put {
                continue;
            }
            match serde_json::from_slice::<SnapshotKvRecord>(&entry.value) {
                Ok(record) => {
                    let owned_key =
                        trogon_sandbox_protocol::tenant_scoped_kv_key(&record.tenant, record.digest.as_str());
                    if key != owned_key && key != trogon_sandbox_protocol::subjects::snapshot_kv_key(&record.digest) {
                        continue;
                    }
                    let identity = (record.tenant.clone(), record.digest.clone());
                    let owned = key == owned_key;
                    if !owned && self.kv.entry(&owned_key).await?.is_some() {
                        continue;
                    }
                    if out.get(&identity).is_none_or(|(_, preferred)| owned || !preferred) {
                        out.insert(identity, (record, owned));
                    }
                }
                Err(e) => {
                    warn!(key = %key, error = %e, "snapshot catalog: skipping undecodable record")
                }
            }
        }
        Ok(out.into_values().map(|(record, _)| record).collect())
    }

    async fn remove(&self, record: &SnapshotKvRecord) -> Result<(), SnapshotCatalogError> {
        for key in [
            trogon_sandbox_protocol::tenant_scoped_kv_key(&record.tenant, record.digest.as_str()),
            trogon_sandbox_protocol::subjects::snapshot_kv_key(&record.digest),
        ] {
            if let Some(entry) = self.kv.entry(&key).await? {
                if entry.operation != kv::Operation::Put {
                    continue;
                }
                let stored: SnapshotKvRecord = serde_json::from_slice(&entry.value)?;
                if stored == *record {
                    self.kv.delete_expect_revision(&key, Some(entry.revision)).await?;
                }
            }
        }
        Ok(())
    }
}
