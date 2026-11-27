use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};

use super::constants::CAPTURE_SLACK;
use crate::SnapshotLifecycleError;
use crate::adapter::E2bWorkerProvider;
use crate::snapshot_custody::{CustodyPhase, CustodyRecord, StoredCustody, after_ms};

impl E2bWorkerProvider {
    /// Takes custody of `reference` before the capture call, so a crash mid-capture leaves a record to clean up.
    pub(crate) async fn begin_snapshot_capture(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
    ) -> Result<StoredCustody, SnapshotLifecycleError> {
        self.snapshots.ready().await?;
        if self.snapshots.load_for_delete(tenant, reference).await?.is_some() {
            return Err(SnapshotLifecycleError::MetadataExists);
        }
        let until = after_ms(self.api.timeout().saturating_mul(4).saturating_add(CAPTURE_SLACK))?;
        Ok(self
            .custody
            .create(CustodyRecord::capturing(tenant.clone(), reference.clone(), until))
            .await?)
    }

    /// Marks a recorded snapshot as kept so cleanup never deletes it.
    pub(crate) async fn keep_snapshot(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
    ) -> Result<(), SnapshotLifecycleError> {
        let stored = self
            .custody
            .load(tenant, reference)
            .await?
            .ok_or(SnapshotLifecycleError::CustodyMissing)?;
        if !matches!(stored.record.phase, CustodyPhase::Owned | CustodyPhase::Kept) {
            return Err(SnapshotLifecycleError::CleanupStarted);
        }
        let mut record = stored.record;
        record.phase = CustodyPhase::Kept;
        record.claim_until_unix_ms = 0;
        self.custody.save(record, stored.revision).await?;
        Ok(())
    }
}
