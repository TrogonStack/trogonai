use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, SnapshotCaptureStatus};

use super::snapshot_capture_status_error::SnapshotCaptureStatusError;

#[async_trait]
pub trait SnapshotCaptureStatusStore: Send + Sync {
    async fn record(&self, lease_id: &LeaseId, status: SnapshotCaptureStatus)
    -> Result<(), SnapshotCaptureStatusError>;
    async fn get(&self, lease_id: &LeaseId) -> Result<Option<SnapshotCaptureStatus>, SnapshotCaptureStatusError>;
}
