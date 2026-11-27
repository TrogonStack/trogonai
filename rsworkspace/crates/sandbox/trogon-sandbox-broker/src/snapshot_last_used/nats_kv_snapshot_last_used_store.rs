use std::time::SystemTime;

use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::Digest;
use trogon_sandbox_protocol::subjects::{SNAPSHOT_LAST_USED_KV_BUCKET, snapshot_kv_key};

use super::snapshot_last_used_error::SnapshotLastUsedError;
use super::snapshot_last_used_store::SnapshotLastUsedStore;

pub struct NatsKvSnapshotLastUsedStore {
    kv: kv::Store,
}

impl NatsKvSnapshotLastUsedStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, SnapshotLastUsedError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: SNAPSHOT_LAST_USED_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl SnapshotLastUsedStore for NatsKvSnapshotLastUsedStore {
    async fn touch(&self, digest: &Digest, at: SystemTime) -> Result<(), SnapshotLastUsedError> {
        let key = snapshot_kv_key(digest);
        let payload = Bytes::from(serde_json::to_vec(&at)?);
        self.kv.put(key, payload).await?;
        Ok(())
    }

    async fn get(&self, digest: &Digest) -> Result<Option<SystemTime>, SnapshotLastUsedError> {
        let key = snapshot_kv_key(digest);
        match self.kv.entry(&key).await? {
            None => Ok(None),
            Some(entry) => {
                let at: SystemTime = serde_json::from_slice(&entry.value)?;
                Ok(Some(at))
            }
        }
    }
}
