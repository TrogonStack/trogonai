use std::sync::Arc;

use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use futures::StreamExt;
use trogon_sandbox_contract::{Digest, LiveSnapshotEntry, TenantId};
use trogon_sandbox_protocol::subjects::SNAPSHOT_LIVE_KV_BUCKET;
use trogon_sandbox_protocol::{parse_tenant_scoped_kv_key_any, tenant_scoped_kv_key};

use super::live_snapshot_store::LiveSnapshotStore;
use super::snapshot_live_error::SnapshotLiveError;
use super::stored_live::{StoredLive, replace};
use crate::constants::{
    SNAPSHOT_LIVE_BUCKET_MAX_BYTES, SNAPSHOT_LIVE_CAS_RETRIES, SNAPSHOT_LIVE_MAX_TENANT_BYTES,
    SNAPSHOT_LIVE_MAX_VALUE_BYTES,
};

pub struct NatsKvLiveSnapshotStore {
    kv: kv::Store,
}

impl NatsKvLiveSnapshotStore {
    pub async fn open(js: jetstream::Context) -> Result<Arc<Self>, SnapshotLiveError> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: SNAPSHOT_LIVE_KV_BUCKET.to_string(),
                history: 1,
                max_bytes: SNAPSHOT_LIVE_BUCKET_MAX_BYTES,
                max_value_size: SNAPSHOT_LIVE_MAX_VALUE_BYTES,
                storage: jetstream::stream::StorageType::File,
                ..Default::default()
            })
            .await?;
        let config = &kv.stream.cached_info().config;
        if !(config.max_age.is_zero() && config.discard == jetstream::stream::DiscardPolicy::New) {
            return Err(SnapshotLiveError::EvictionPolicy);
        }
        Ok(Arc::new(Self { kv }))
    }

    async fn row(&self, tenant: &TenantId, digest: &Digest) -> Result<Option<(StoredLive, u64)>, SnapshotLiveError> {
        let Some(entry) = self.kv.entry(tenant_scoped_kv_key(tenant, digest.as_str())).await? else {
            return Ok(None);
        };
        if entry.operation != kv::Operation::Put {
            return Ok(None);
        }
        let row: StoredLive = serde_json::from_slice(&entry.value)?;
        if row.entry.tenant != *tenant {
            return Err(SnapshotLiveError::OwnerMismatch);
        }
        Ok(Some((row, entry.revision)))
    }

    async fn write(&self, digest: &Digest, next: StoredLive) -> Result<(), SnapshotLiveError> {
        if next.entry.tenant.as_str().len() > SNAPSHOT_LIVE_MAX_TENANT_BYTES {
            return Err(SnapshotLiveError::TenantTooLong);
        }
        let key = tenant_scoped_kv_key(&next.entry.tenant, digest.as_str());
        let payload = serde_json::to_vec(&next)?;
        for _ in 0..SNAPSHOT_LIVE_CAS_RETRIES {
            match self.row(&next.entry.tenant, digest).await? {
                Some((current, revision)) => {
                    if !replace(&current, &next)? {
                        return Ok(());
                    }
                    match self.kv.update(&key, payload.clone().into(), revision).await {
                        Ok(_) => return Ok(()),
                        Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => {
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                None => match self.kv.create(&key, payload.clone().into()).await {
                    Ok(_) => return Ok(()),
                    Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error.into()),
                },
            }
        }
        Err(SnapshotLiveError::RetriesExhausted)
    }
}

#[async_trait]
impl LiveSnapshotStore for NatsKvLiveSnapshotStore {
    async fn put(&self, digest: &Digest, entry: &LiveSnapshotEntry) -> Result<(), SnapshotLiveError> {
        self.put_at(digest, entry, 0).await
    }
    async fn put_at(&self, digest: &Digest, entry: &LiveSnapshotEntry, revision: u64) -> Result<(), SnapshotLiveError> {
        self.write(
            digest,
            StoredLive {
                entry: entry.clone(),
                revision,
                deleted: false,
            },
        )
        .await
    }
    async fn get_owned(
        &self,
        tenant: &TenantId,
        digest: &Digest,
    ) -> Result<Option<LiveSnapshotEntry>, SnapshotLiveError> {
        Ok(self
            .row(tenant, digest)
            .await?
            .filter(|(row, _)| !row.deleted)
            .map(|(row, _)| row.entry))
    }
    async fn get(&self, digest: &Digest) -> Result<Option<LiveSnapshotEntry>, SnapshotLiveError> {
        let mut keys = self.kv.keys().await?;
        let mut found = None;
        while let Some(key) = keys.next().await {
            if let Some((tenant, name)) = parse_tenant_scoped_kv_key_any(&key?)
                && name == digest.as_str()
                && let Some(entry) = self.get_owned(&tenant, digest).await?
            {
                if found.is_some() {
                    return Err(SnapshotLiveError::AmbiguousDigest);
                }
                found = Some(entry);
            }
        }
        Ok(found)
    }
    async fn remove(&self, digest: &Digest) -> Result<(), SnapshotLiveError> {
        let Some(entry) = self.get(digest).await? else {
            return Ok(());
        };
        let revision = self
            .row(&entry.tenant, digest)
            .await?
            .map_or(0, |(row, _)| row.revision)
            .checked_add(1)
            .ok_or(SnapshotLiveError::RevisionOverflow)?;
        self.remove_at(digest, &entry, revision).await
    }
    async fn remove_at(
        &self,
        digest: &Digest,
        entry: &LiveSnapshotEntry,
        revision: u64,
    ) -> Result<(), SnapshotLiveError> {
        self.write(
            digest,
            StoredLive {
                entry: entry.clone(),
                revision,
                deleted: true,
            },
        )
        .await
    }
}
