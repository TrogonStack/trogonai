use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{Digest, LeaseId};
use trogon_sandbox_protocol::subjects::LEASE_RESOLVED_DIGEST_KV_BUCKET;

use super::{ResolvedDigestError, ResolvedDigestStore};

fn lease_kv_key(lease_id: &LeaseId) -> String {
    lease_id.as_str().replace([':', '/', '.'], "_")
}

pub struct NatsKvResolvedDigestStore {
    kv: kv::Store,
}

impl NatsKvResolvedDigestStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, ResolvedDigestError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: LEASE_RESOLVED_DIGEST_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(ResolvedDigestError::backend)?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl ResolvedDigestStore for NatsKvResolvedDigestStore {
    async fn record(&self, lease_id: &LeaseId, digest: &Digest) -> Result<(), ResolvedDigestError> {
        let payload = Bytes::from(serde_json::to_vec(digest)?);
        self.kv
            .put(lease_kv_key(lease_id), payload)
            .await
            .map_err(ResolvedDigestError::backend)?;
        Ok(())
    }

    async fn get(&self, lease_id: &LeaseId) -> Result<Option<Digest>, ResolvedDigestError> {
        match self
            .kv
            .entry(&lease_kv_key(lease_id))
            .await
            .map_err(ResolvedDigestError::backend)?
        {
            Some(entry) if entry.operation == kv::Operation::Put => Ok(Some(serde_json::from_slice(&entry.value)?)),
            Some(_) | None => Ok(None),
        }
    }

    async fn clear(&self, lease_id: &LeaseId) -> Result<(), ResolvedDigestError> {
        self.kv
            .delete(&lease_kv_key(lease_id))
            .await
            .map_err(ResolvedDigestError::backend)?;
        Ok(())
    }
}
