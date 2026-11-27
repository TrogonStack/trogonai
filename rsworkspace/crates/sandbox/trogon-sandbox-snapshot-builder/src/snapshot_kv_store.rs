use std::collections::HashSet;

use async_nats::jetstream::kv::{self, CreateErrorKind, Operation, UpdateErrorKind};
use async_nats::jetstream::{self};
use futures::StreamExt;
use trogon_sandbox_contract::{Digest, SnapshotKvRecord, TenantId};
use trogon_sandbox_protocol::subjects::{SNAPSHOT_KV_BUCKET, snapshot_kv_key};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use crate::SnapshotKvError;
use crate::constants::KV_CAS_ATTEMPTS;

/// The bucket that maps a snapshot digest to the lease and tenant that produced it.
pub struct SnapshotKvStore {
    kv: kv::Store,
}

impl SnapshotKvStore {
    pub async fn open(jetstream: jetstream::Context) -> Result<Self, SnapshotKvError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: SNAPSHOT_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }

    /// Records `record` unless a newer or identical one is already there.
    pub async fn put(&self, record: &SnapshotKvRecord) -> Result<(), SnapshotKvError> {
        let key = tenant_scoped_kv_key(&record.tenant, record.digest.as_str());
        let payload = serde_json::to_vec(record)?;
        for _ in 0..KV_CAS_ATTEMPTS {
            match self.kv.entry(&key).await? {
                Some(entry) => {
                    if entry.operation == Operation::Put {
                        let current: SnapshotKvRecord = serde_json::from_slice(&entry.value)?;
                        if current.tenant != record.tenant || current.digest != record.digest {
                            return Err(SnapshotKvError::IdentityMismatch);
                        }
                        if current.created_at > record.created_at || current == *record {
                            return Ok(());
                        }
                        if current.created_at == record.created_at {
                            return Err(SnapshotKvError::ConflictingMetadata);
                        }
                    }
                    match self.kv.update(&key, payload.clone().into(), entry.revision).await {
                        Ok(_) => return Ok(()),
                        Err(error) if error.kind() == UpdateErrorKind::WrongLastRevision => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                None => match self.kv.create(&key, payload.clone().into()).await {
                    Ok(_) => return Ok(()),
                    Err(error) if error.kind() == CreateErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error.into()),
                },
            }
        }
        Err(SnapshotKvError::RetriesExhausted)
    }

    /// The record of `digest` when `tenant` owns it.
    pub async fn get_owned(
        &self,
        tenant: &TenantId,
        digest: &Digest,
    ) -> Result<Option<SnapshotKvRecord>, SnapshotKvError> {
        let key = tenant_scoped_kv_key(tenant, digest.as_str());
        if let Some(entry) = self.kv.entry(&key).await? {
            if entry.operation != Operation::Put {
                return Ok(None);
            }
            let record: SnapshotKvRecord = serde_json::from_slice(&entry.value)?;
            if record.tenant != *tenant || record.digest != *digest {
                return Err(SnapshotKvError::IdentityMismatch);
            }
            return Ok(Some(record));
        }
        if let Some(entry) = self.kv.entry(snapshot_kv_key(digest)).await?
            && entry.operation == Operation::Put
        {
            let record: SnapshotKvRecord = serde_json::from_slice(&entry.value)?;
            return Ok((record.tenant == *tenant && record.digest == *digest).then_some(record));
        }
        Ok(None)
    }

    /// The record of `digest` when exactly one tenant holds it.
    pub async fn get(&self, digest: &Digest) -> Result<Option<SnapshotKvRecord>, SnapshotKvError> {
        let mut keys = self.kv.keys().await?;
        let mut owners = HashSet::new();
        while let Some(key) = keys.next().await {
            let key = key?;
            if let Some(entry) = self.kv.entry(&key).await? {
                if entry.operation != Operation::Put {
                    continue;
                }
                let record: SnapshotKvRecord = serde_json::from_slice(&entry.value)?;
                if record.digest == *digest {
                    owners.insert(record.tenant);
                }
            }
        }
        if owners.len() > 1 {
            return Err(SnapshotKvError::TenantRequired);
        }
        match owners.into_iter().next() {
            Some(tenant) => self.get_owned(&tenant, digest).await,
            None => Ok(None),
        }
    }
}
