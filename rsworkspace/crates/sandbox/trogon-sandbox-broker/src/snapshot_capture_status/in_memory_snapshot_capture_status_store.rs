use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{LeaseId, SnapshotCaptureStatus};

use super::snapshot_capture_status_error::SnapshotCaptureStatusError;
use super::snapshot_capture_status_store::SnapshotCaptureStatusStore;

pub struct InMemorySnapshotCaptureStatusStore {
    records: RwLock<HashMap<String, SnapshotCaptureStatus>>,
}

impl InMemorySnapshotCaptureStatusStore {
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemorySnapshotCaptureStatusStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SnapshotCaptureStatusStore for InMemorySnapshotCaptureStatusStore {
    async fn record(
        &self,
        lease_id: &LeaseId,
        status: SnapshotCaptureStatus,
    ) -> Result<(), SnapshotCaptureStatusError> {
        self.records.write().await.insert(lease_id.as_str().to_owned(), status);
        Ok(())
    }

    async fn get(&self, lease_id: &LeaseId) -> Result<Option<SnapshotCaptureStatus>, SnapshotCaptureStatusError> {
        Ok(self.records.read().await.get(lease_id.as_str()).cloned())
    }
}
