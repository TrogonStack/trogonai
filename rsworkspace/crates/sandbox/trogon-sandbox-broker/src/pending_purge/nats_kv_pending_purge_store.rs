use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::subjects::LEASE_PENDING_PURGE_KV_BUCKET;

use super::{PENDING_PURGE_MAX_AGE, PendingPurge, PendingPurgeError, PendingPurgeStore};

fn lease_kv_key(lease_id: &LeaseId) -> String {
    lease_id.as_str().replace([':', '/', '.'], "_")
}

pub struct NatsKvPendingPurgeStore {
    kv: kv::Store,
}

impl NatsKvPendingPurgeStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, PendingPurgeError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: LEASE_PENDING_PURGE_KV_BUCKET.to_string(),
                history: 1,
                max_age: PENDING_PURGE_MAX_AGE,
                ..Default::default()
            })
            .await
            .map_err(PendingPurgeError::nats)?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl PendingPurgeStore for NatsKvPendingPurgeStore {
    async fn put(&self, purge: &PendingPurge) -> Result<(), PendingPurgeError> {
        let payload = Bytes::from(serde_json::to_vec(purge)?);
        self.kv
            .put(lease_kv_key(&purge.lease_id), payload)
            .await
            .map_err(PendingPurgeError::nats)?;
        Ok(())
    }

    async fn get(&self, lease_id: &LeaseId) -> Result<Option<PendingPurge>, PendingPurgeError> {
        match self
            .kv
            .get(lease_kv_key(lease_id))
            .await
            .map_err(PendingPurgeError::nats)?
        {
            None => Ok(None),
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        }
    }

    async fn clear(&self, lease_id: &LeaseId) -> Result<(), PendingPurgeError> {
        self.kv
            .purge(lease_kv_key(lease_id))
            .await
            .map_err(PendingPurgeError::nats)?;
        Ok(())
    }

    async fn all(&self) -> Result<Vec<PendingPurge>, PendingPurgeError> {
        let mut keys = self.kv.keys().await.map_err(PendingPurgeError::nats)?;
        let mut out = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(PendingPurgeError::nats)?;
            if let Some(bytes) = self.kv.get(&key).await.map_err(PendingPurgeError::nats)? {
                out.push(serde_json::from_slice(&bytes)?);
            }
        }
        Ok(out)
    }
}
