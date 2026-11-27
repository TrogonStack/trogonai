use super::constants::MAX_CLEANUPS_PER_PASS;
use crate::SnapshotLifecycleError;
use crate::adapter::E2bWorkerProvider;
use crate::snapshot_custody::{CustodyPhase, MAX_AUTOMATIC_ATTEMPTS, now_ms};

impl E2bWorkerProvider {
    /// Settles custody left behind by a crash: an expired capture claim becomes kept or cleanup, and due cleanups run.
    pub(crate) async fn recover_snapshot_cleanup(&self) -> Result<(), SnapshotLifecycleError> {
        let records = self.custody.scan().await?;
        let mut attempts = 0;
        for mut stored in records {
            if stored.record.phase == CustodyPhase::Owned && stored.record.claim_until_unix_ms <= now_ms()? {
                let manifest = self
                    .snapshots
                    .load_for_delete(&stored.record.tenant, &stored.record.reference)
                    .await?;
                let mut record = stored.record;
                if let Some(manifest) = manifest {
                    if Some(&manifest.restore_binding()?.template_id) != record.template_id.as_ref() {
                        return Err(SnapshotLifecycleError::RecoveryBindingMismatch);
                    }
                    record.phase = CustodyPhase::Kept;
                } else {
                    record.phase = CustodyPhase::Cleanup;
                }
                record.claim_until_unix_ms = 0;
                stored = self.custody.save(record, stored.revision).await?;
            }
            let now = now_ms()?;
            let due = stored.record.phase == CustodyPhase::Cleanup
                && attempts < MAX_CLEANUPS_PER_PASS
                && stored.record.attempts < MAX_AUTOMATIC_ATTEMPTS
                && stored.record.claim_until_unix_ms <= now
                && stored.record.next_attempt_unix_ms <= now;
            if due {
                attempts += 1;
                if let Err(error) = self.attempt_snapshot_cleanup(stored, false).await {
                    tracing::warn!(%error, "native snapshot cleanup remains in durable custody");
                }
            }
        }
        Ok(())
    }
}
