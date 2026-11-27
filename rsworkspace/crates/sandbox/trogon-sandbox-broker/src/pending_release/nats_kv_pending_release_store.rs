use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::subjects::LEASE_PENDING_RELEASE_KV_BUCKET;

use super::{PENDING_RELEASE_MAX_AGE, PendingRelease, PendingReleaseError, PendingReleaseStore};

fn lease_kv_key(lease_id: &LeaseId) -> String {
    lease_id.as_str().replace([':', '/', '.'], "_")
}

pub struct NatsKvPendingReleaseStore {
    kv: kv::Store,
}

impl NatsKvPendingReleaseStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, PendingReleaseError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: LEASE_PENDING_RELEASE_KV_BUCKET.to_string(),
                history: 1,
                max_age: PENDING_RELEASE_MAX_AGE,
                ..Default::default()
            })
            .await
            .map_err(PendingReleaseError::nats)?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl PendingReleaseStore for NatsKvPendingReleaseStore {
    async fn create(&self, release: &PendingRelease) -> Result<(PendingRelease, bool), PendingReleaseError> {
        let key = lease_kv_key(&release.lease_id);
        let payload = Bytes::from(serde_json::to_vec(release)?);
        match self.kv.create(&key, payload).await {
            Ok(_) => Ok((release.clone(), true)),
            Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => {
                match self.kv.get(&key).await.map_err(PendingReleaseError::nats)? {
                    Some(bytes) => Ok((serde_json::from_slice(&bytes)?, false)),
                    // Cleared between the create and the read: the release
                    // that wrote it finished, so this one is a late repeat.
                    None => Ok((release.clone(), false)),
                }
            }
            Err(e) => Err(PendingReleaseError::nats(e)),
        }
    }

    async fn clear(&self, lease_id: &LeaseId) -> Result<(), PendingReleaseError> {
        self.kv
            .purge(lease_kv_key(lease_id))
            .await
            .map_err(PendingReleaseError::nats)?;
        Ok(())
    }

    async fn all(&self) -> Result<Vec<PendingRelease>, PendingReleaseError> {
        let mut keys = self.kv.keys().await.map_err(PendingReleaseError::nats)?;
        let mut out = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(PendingReleaseError::nats)?;
            if let Some(bytes) = self.kv.get(&key).await.map_err(PendingReleaseError::nats)? {
                out.push(serde_json::from_slice(&bytes)?);
            }
        }
        Ok(out)
    }
}
