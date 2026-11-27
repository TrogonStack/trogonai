use std::time::Duration;

use trogon_sandbox_contract::{NativeSnapshotRef, SnapshotExportError, SnapshotExportOutcome, TenantId};

use super::constants::{CLAIM_ATTEMPTS, CLEANUP_SLACK, MAX_BACKOFF_EXPONENT};
use crate::SnapshotLifecycleError;
use crate::adapter::E2bWorkerProvider;
use crate::api::ApiError;
use crate::snapshot_custody::{CustodyPhase, CustodyRecord, MAX_AUTOMATIC_ATTEMPTS, StoredCustody, after_ms, now_ms};
use crate::snapshots::SnapshotTemplateId;

impl E2bWorkerProvider {
    pub(super) async fn delete_exact_snapshot(
        &self,
        template_id: &SnapshotTemplateId,
    ) -> Result<(), SnapshotLifecycleError> {
        match self.api.delete_snapshot(template_id).await {
            Ok(()) | Err(ApiError::NotFound(_)) => {}
            Err(error) => return Err(error.into()),
        }
        if !self.api.snapshot_is_absent(template_id).await? {
            return Err(SnapshotLifecycleError::AbsenceUnconfirmed);
        }
        Ok(())
    }

    /// Moves the snapshot into the cleanup phase, creating the custody record when none exists.
    pub(super) async fn mark_cleanup(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
        template_id: &SnapshotTemplateId,
    ) -> Result<StoredCustody, SnapshotLifecycleError> {
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(stored) = self.custody.load(tenant, reference).await? else {
                let mut record = CustodyRecord::capturing(tenant.clone(), reference.clone(), 0);
                record.template_id = Some(template_id.clone());
                record.phase = CustodyPhase::Cleanup;
                match self.custody.create(record).await {
                    Ok(stored) => return Ok(stored),
                    Err(error) if self.custody.load(tenant, reference).await?.is_none() => return Err(error.into()),
                    Err(_) => continue,
                }
            };
            if stored.record.template_id.as_ref().is_some_and(|id| id != template_id) {
                return Err(SnapshotLifecycleError::CleanupIdentityMismatch);
            }
            if matches!(stored.record.phase, CustodyPhase::Cleanup | CustodyPhase::Deleted) {
                return Ok(stored);
            }
            let mut record = stored.record;
            record.template_id = Some(template_id.clone());
            record.phase = CustodyPhase::Cleanup;
            record.claim_until_unix_ms = 0;
            record.next_attempt_unix_ms = 0;
            if let Ok(stored) = self.custody.save(record, stored.revision).await {
                return Ok(stored);
            }
        }
        Err(SnapshotLifecycleError::ClaimExhausted)
    }

    /// Deletes the template of a snapshot in cleanup. `Ok(false)` means another claim or a backoff holds it.
    pub(super) async fn attempt_snapshot_cleanup(
        &self,
        stored: StoredCustody,
        manual: bool,
    ) -> Result<bool, SnapshotLifecycleError> {
        if stored.record.phase == CustodyPhase::Deleted {
            return Ok(true);
        }
        if stored.record.phase != CustodyPhase::Cleanup {
            return Err(SnapshotLifecycleError::NotPendingCleanup);
        }
        let now = now_ms()?;
        let backing_off = stored.record.attempts >= MAX_AUTOMATIC_ATTEMPTS || stored.record.next_attempt_unix_ms > now;
        if stored.record.claim_until_unix_ms > now || (!manual && backing_off) {
            return Ok(false);
        }
        let mut record = stored.record;
        record.attempts = record.attempts.saturating_add(1);
        record.claim_until_unix_ms = after_ms(self.api.timeout().saturating_mul(2).saturating_add(CLEANUP_SLACK))?;
        let claimed = self.custody.save(record, stored.revision).await?;
        let template = claimed
            .record
            .template_id
            .as_ref()
            .ok_or(SnapshotLifecycleError::CleanupTemplateMissing)?;
        let result = self.delete_exact_snapshot(template).await;
        let mut record = claimed.record;
        record.claim_until_unix_ms = 0;
        if result.is_ok() {
            record.phase = CustodyPhase::Deleted;
            record.next_attempt_unix_ms = 0;
        } else {
            record.next_attempt_unix_ms =
                after_ms(Duration::from_secs(1u64 << record.attempts.min(MAX_BACKOFF_EXPONENT)))?;
        }
        let saved = self.custody.save(record, claimed.revision).await?;
        if result.is_ok() {
            self.remove_matching_manifest(&saved).await?;
        }
        result.map(|()| true)
    }

    /// Drops the manifest of a deleted snapshot when it still binds the template that was deleted.
    async fn remove_matching_manifest(&self, deleted: &StoredCustody) -> Result<(), SnapshotLifecycleError> {
        let record = &deleted.record;
        let Some(manifest) = self
            .snapshots
            .load_for_delete(&record.tenant, &record.reference)
            .await?
        else {
            return Ok(());
        };
        let binds_deleted =
            manifest.restore_binding().ok().map(|binding| &binding.template_id) == record.template_id.as_ref();
        if binds_deleted {
            self.snapshots.remove(&record.tenant, &record.reference).await?;
        }
        Ok(())
    }

    /// Reports a failed capture as transient after trying to delete the snapshot it left behind.
    pub(crate) async fn snapshot_cleanup(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
        template_id: &SnapshotTemplateId,
        primary: String,
    ) -> SnapshotExportOutcome {
        let cleanup = match self.mark_cleanup(tenant, reference, template_id).await {
            Ok(stored) => self.attempt_snapshot_cleanup(stored, true).await.map(|_| ()),
            Err(custody) => Err(SnapshotLifecycleError::CustodyPersistence {
                custody: Box::new(custody),
                cleanup: self.delete_exact_snapshot(template_id).await.map_err(Box::new),
            }),
        };
        let message = match cleanup {
            Ok(()) => primary,
            Err(error) => format!(
                "{primary}; snapshot cleanup remains unresolved: {error}; retained owned snapshot template_id={}",
                template_id.as_str()
            ),
        };
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { message })
    }
}
