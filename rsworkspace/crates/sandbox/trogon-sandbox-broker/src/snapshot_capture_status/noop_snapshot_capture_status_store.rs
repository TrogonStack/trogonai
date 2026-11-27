use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, SnapshotCaptureStatus};

use super::snapshot_capture_status_error::SnapshotCaptureStatusError;
use super::snapshot_capture_status_store::SnapshotCaptureStatusStore;

pub struct NoopSnapshotCaptureStatusStore;

#[async_trait]
impl SnapshotCaptureStatusStore for NoopSnapshotCaptureStatusStore {
    async fn record(
        &self,
        _lease_id: &LeaseId,
        _status: SnapshotCaptureStatus,
    ) -> Result<(), SnapshotCaptureStatusError> {
        Ok(())
    }

    async fn get(&self, _lease_id: &LeaseId) -> Result<Option<SnapshotCaptureStatus>, SnapshotCaptureStatusError> {
        Ok(None)
    }
}
