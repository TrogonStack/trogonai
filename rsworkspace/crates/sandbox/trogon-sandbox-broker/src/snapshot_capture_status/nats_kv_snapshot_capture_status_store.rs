use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{LeaseId, SnapshotCaptureStatus};
use trogon_sandbox_protocol::subjects::SNAPSHOT_CAPTURE_STATUS_KV_BUCKET;

use super::snapshot_capture_status_error::SnapshotCaptureStatusError;
use super::snapshot_capture_status_store::SnapshotCaptureStatusStore;

fn lease_kv_key(lease_id: &LeaseId) -> String {
    lease_id.as_str().replace([':', '/', '.'], "_")
}

pub struct NatsKvSnapshotCaptureStatusStore {
    kv: kv::Store,
}

impl NatsKvSnapshotCaptureStatusStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, SnapshotCaptureStatusError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: SNAPSHOT_CAPTURE_STATUS_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl SnapshotCaptureStatusStore for NatsKvSnapshotCaptureStatusStore {
    async fn record(
        &self,
        lease_id: &LeaseId,
        status: SnapshotCaptureStatus,
    ) -> Result<(), SnapshotCaptureStatusError> {
        let key = lease_kv_key(lease_id);
        let payload = Bytes::from(serde_json::to_vec(&status)?);
        self.kv.put(key, payload).await?;
        Ok(())
    }

    async fn get(&self, lease_id: &LeaseId) -> Result<Option<SnapshotCaptureStatus>, SnapshotCaptureStatusError> {
        let key = lease_kv_key(lease_id);
        match self.kv.entry(&key).await? {
            None => Ok(None),
            Some(entry) => {
                let status: SnapshotCaptureStatus = serde_json::from_slice(&entry.value)?;
                Ok(Some(status))
            }
        }
    }
}
