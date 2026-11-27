use trogon_sandbox_contract::{LeaseError, NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse};

use crate::SnapshotLifecycleError;
use crate::adapter::E2bWorkerProvider;
use crate::snapshot_custody::{CustodyPhase, StoredCustody};
use crate::snapshots::validate_reference_identity;

impl E2bWorkerProvider {
    /// Deletes the snapshot a tenant owns, including one whose reference has expired.
    pub async fn native_snapshot_delete(&self, request: &NativeSnapshotDeleteRequest) -> NativeSnapshotDeleteResponse {
        match self.delete_native_snapshot(request).await {
            Ok(response) => response,
            Err(error) => NativeSnapshotDeleteResponse::Denied(LeaseError::Backend {
                message: error.to_string(),
            }),
        }
    }

    async fn delete_native_snapshot(
        &self,
        request: &NativeSnapshotDeleteRequest,
    ) -> Result<NativeSnapshotDeleteResponse, SnapshotLifecycleError> {
        validate_reference_identity(&request.reference)?;
        let gate = self.snapshot_gate(&request.tenant, &request.reference);
        let _held = gate.lock().await;
        if let Some(stored) = self.custody.load(&request.tenant, &request.reference).await? {
            match stored.record.phase {
                CustodyPhase::Deleted => return Ok(NativeSnapshotDeleteResponse::NotFound),
                CustodyPhase::Cleanup => {
                    return self.finish_cleanup(stored).await;
                }
                _ => {}
            }
        }
        let Some(manifest) = self
            .snapshots
            .load_for_delete(&request.tenant, &request.reference)
            .await?
        else {
            return Ok(NativeSnapshotDeleteResponse::NotFound);
        };
        let template = &manifest.restore_binding()?.template_id;
        let stored = self.mark_cleanup(&request.tenant, &request.reference, template).await?;
        self.finish_cleanup(stored).await
    }

    async fn finish_cleanup(
        &self,
        stored: StoredCustody,
    ) -> Result<NativeSnapshotDeleteResponse, SnapshotLifecycleError> {
        if !self.attempt_snapshot_cleanup(stored, true).await? {
            return Err(SnapshotLifecycleError::CleanupInProgress);
        }
        Ok(NativeSnapshotDeleteResponse::Deleted)
    }
}
