use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};
use trogon_sandbox_protocol::subjects::NATIVE_SNAPSHOT_KV_BUCKET;
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::NativeSnapshotStore;
use super::constants::CAS_RETRIES;
use super::owned_snapshot::{OwnedSnapshot, SnapshotState, entry_name};

pub struct NatsKvNativeSnapshotStore {
    kv: kv::Store,
}

impl NatsKvNativeSnapshotStore {
    pub async fn new(jetstream: jetstream::Context) -> anyhow::Result<Self> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: NATIVE_SNAPSHOT_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }

    async fn load(
        &self,
        tenant: &TenantId,
        snapshot: &NativeSnapshotRef,
    ) -> anyhow::Result<Option<(OwnedSnapshot, u64)>> {
        let key = tenant_scoped_kv_key(tenant, &entry_name(snapshot));
        match self.kv.entry(&key).await? {
            Some(entry) if entry.operation == kv::Operation::Put => Ok(Some((
                OwnedSnapshot::decode(&entry.value, tenant, snapshot)?,
                entry.revision,
            ))),
            _ => Ok(None),
        }
    }
}

#[async_trait]
impl NativeSnapshotStore for NatsKvNativeSnapshotStore {
    async fn record(&self, tenant: &TenantId, snapshot: &NativeSnapshotRef) -> anyhow::Result<()> {
        let record = OwnedSnapshot::live(tenant, snapshot);
        let key = tenant_scoped_kv_key(tenant, &entry_name(snapshot));
        let data = Bytes::from(serde_json::to_vec(&record)?);
        for _ in 0..CAS_RETRIES {
            if let Some((existing, revision)) = self.load(tenant, snapshot).await? {
                anyhow::ensure!(
                    existing.state == SnapshotState::Live,
                    "native snapshot identity is already retired"
                );
                if existing.reference == *snapshot {
                    return Ok(());
                }
                match self.kv.update(&key, data.clone(), revision).await {
                    Ok(_) => return Ok(()),
                    Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => {
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                }
            } else {
                match self.kv.create(&key, data.clone()).await {
                    Ok(_) => return Ok(()),
                    Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        anyhow::bail!("native snapshot identity changed too often while recording")
    }

    async fn owns(&self, tenant: &TenantId, snapshot: &NativeSnapshotRef) -> anyhow::Result<bool> {
        Ok(self
            .load(tenant, snapshot)
            .await?
            .is_some_and(|(record, _)| record.state == SnapshotState::Live))
    }

    async fn begin_delete(
        &self,
        tenant: &TenantId,
        snapshot: &NativeSnapshotRef,
    ) -> anyhow::Result<Option<NativeSnapshotRef>> {
        let key = tenant_scoped_kv_key(tenant, &entry_name(snapshot));
        for _ in 0..CAS_RETRIES {
            let Some((mut record, revision)) = self.load(tenant, snapshot).await? else {
                return Ok(None);
            };
            match record.state {
                SnapshotState::Deleted => return Ok(None),
                SnapshotState::Deleting => return Ok(Some(record.reference)),
                SnapshotState::Live => record.state = SnapshotState::Deleting,
            }
            match self
                .kv
                .update(&key, Bytes::from(serde_json::to_vec(&record)?), revision)
                .await
            {
                Ok(_) => return Ok(Some(record.reference)),
                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("native snapshot deletion intent changed too often")
    }

    async fn finish_delete(&self, tenant: &TenantId, snapshot: &NativeSnapshotRef) -> anyhow::Result<()> {
        let key = tenant_scoped_kv_key(tenant, &entry_name(snapshot));
        for _ in 0..CAS_RETRIES {
            let (mut record, revision) = self
                .load(tenant, snapshot)
                .await?
                .ok_or_else(|| anyhow::anyhow!("native snapshot deletion intent is missing"))?;
            match record.state {
                SnapshotState::Deleted => return Ok(()),
                SnapshotState::Deleting => record.state = SnapshotState::Deleted,
                SnapshotState::Live => anyhow::bail!("native snapshot deletion intent is missing"),
            }
            match self
                .kv
                .update(&key, Bytes::from(serde_json::to_vec(&record)?), revision)
                .await
            {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("native snapshot deletion completion changed too often")
    }
}
