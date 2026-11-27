use trogon_sandbox_contract::{
    Digest, LiveSnapshotEntry, SnapshotAccountingRefusal as Refusal, SnapshotDeleted, TenantId,
};

use super::refusal_result::Result;
use super::snapshot_accounting_store::SnapshotAccountingStore;
use super::snapshot_deletion_claim::SnapshotDeletionClaim;
use crate::constants::{
    SNAPSHOT_ACCOUNTING_MAX_RECEIPTS, SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES, SNAPSHOT_ACCOUNTING_ROW_HEADROOM_BYTES,
};

impl SnapshotAccountingStore {
    pub async fn claim_delete(&self, tenant: &TenantId, digest: &Digest) -> Result<Option<SnapshotDeletionClaim>> {
        let claim_id = crate::generated_ulid::generated_ulid();
        self.mutate(tenant, |row| {
            if row
                .fences
                .get(digest)
                .and_then(|fence| fence.pending_delete.as_ref())
                .is_some()
            {
                return Err(Refusal::Pending);
            }
            if row
                .captures
                .values()
                .any(|capture| !capture.abandoned && capture.binding.is_none())
            {
                return Err(Refusal::Pending);
            }
            if row.deletions.len() >= SNAPSHOT_ACCOUNTING_MAX_RECEIPTS {
                return Err(Refusal::Capacity);
            }
            let capture = match row.live.get(digest).and_then(|live| live.entry.generation.as_ref()) {
                Some(generation) => row.capture(generation)?,
                None => {
                    let closed = row.fences.get(digest).map_or(0, |fence| fence.closed_through);
                    let Some(capture) = row
                        .captures
                        .values()
                        .filter(|capture| {
                            !capture.abandoned
                                && capture.reservation.sequence > closed
                                && capture
                                    .binding
                                    .as_ref()
                                    .is_some_and(|binding| binding.reference.digest == *digest)
                        })
                        .max_by_key(|capture| capture.reservation.sequence)
                        .cloned()
                    else {
                        return Ok(None);
                    };
                    capture
                }
            };
            let binding = capture.binding.as_ref().ok_or(Refusal::Pending)?;
            let live = row.live.get(digest);
            let charged = live.is_some();
            let entry = live
                .map(|live| live.entry.clone())
                .unwrap_or_else(|| LiveSnapshotEntry {
                    tenant: tenant.clone(),
                    artifact_type: binding.reference.artifact_type,
                    published_at: capture.reservation.created_at,
                    bytes: binding.bytes,
                    generation: Some(capture.reservation.generation.clone()),
                });
            let claim = SnapshotDeletionClaim {
                tenant: tenant.clone(),
                reference: binding.reference.clone(),
                generation: capture.reservation.generation.clone(),
                lease_id: capture.reservation.lease_id.clone(),
                claim_id: claim_id.clone(),
                sequence: row.next()?,
                entry,
                charged,
                completed: false,
                projection_revision: 0,
            };
            row.fences.entry(digest.clone()).or_default().pending_delete = Some(claim_id.clone());
            row.deletions.insert(claim_id.clone(), claim.clone());
            if row.encode()?.len() > SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES - SNAPSHOT_ACCOUNTING_ROW_HEADROOM_BYTES {
                return Err(Refusal::Capacity);
            }
            Ok(Some(claim))
        })
        .await
    }

    pub async fn complete_delete(&self, claim: &SnapshotDeletionClaim) -> Result<SnapshotDeletionClaim> {
        self.mutate(&claim.tenant, |row| {
            let stored = row
                .deletions
                .get(&claim.claim_id)
                .cloned()
                .ok_or(Refusal::UnknownGeneration)?;
            if stored.completed {
                return Ok(stored);
            }
            if stored != *claim {
                return Err(Refusal::MetadataMismatch);
            }
            if stored.charged {
                let live = row.live.get(&claim.reference.digest).ok_or(Refusal::MetadataMismatch)?;
                if live.entry.generation.as_ref() != Some(&claim.generation) {
                    return Err(Refusal::MetadataMismatch);
                }
                let quota = row.quota.as_mut().ok_or(Refusal::Backend)?;
                quota.count_used = quota.count_used.checked_sub(1).ok_or(Refusal::MetadataMismatch)?;
                quota.bytes_used = quota
                    .bytes_used
                    .checked_sub(live.entry.bytes)
                    .ok_or(Refusal::MetadataMismatch)?;
                row.live.remove(&claim.reference.digest);
            }
            let revision = row.next()?;
            let fence = row.fences.get_mut(&claim.reference.digest).ok_or(Refusal::Backend)?;
            if fence.pending_delete.as_deref() != Some(claim.claim_id.as_str()) {
                return Err(Refusal::MetadataMismatch);
            }
            fence.closed_through = fence.closed_through.max(claim.sequence);
            fence.pending_delete = None;
            let completed = row.deletions.get_mut(&claim.claim_id).ok_or(Refusal::Backend)?;
            completed.completed = true;
            completed.projection_revision = revision;
            Ok(completed.clone())
        })
        .await
    }

    pub async fn deleted_receipt(&self, event: &SnapshotDeleted) -> Result<SnapshotDeletionClaim> {
        let generation = event.generation.as_ref().ok_or(Refusal::LegacyUnconfirmed)?;
        let row = self.read(&event.tenant).await?.ok_or(Refusal::UnknownGeneration)?;
        let claim = row
            .deletions
            .values()
            .find(|claim| claim.generation == *generation && claim.reference.digest == event.digest)
            .cloned()
            .ok_or(Refusal::UnknownGeneration)?;
        if !claim.completed {
            return Err(Refusal::Pending);
        }
        if claim.tenant != event.tenant || claim.entry.bytes != event.bytes {
            return Err(Refusal::MetadataMismatch);
        }
        Ok(claim)
    }

    pub async fn pending_deletions(&self, tenant: &TenantId) -> Result<Vec<SnapshotDeletionClaim>> {
        Ok(self
            .read(tenant)
            .await?
            .map(|row| row.deletions.into_values().filter(|claim| !claim.completed).collect())
            .unwrap_or_default())
    }

    pub async fn projection(
        &self,
        tenant: &TenantId,
        digest: &Digest,
    ) -> Result<Option<(LiveSnapshotEntry, u64, bool)>> {
        let Some(row) = self.read(tenant).await? else {
            return Ok(None);
        };
        if let Some(live) = row.live.get(digest) {
            return Ok(Some((live.entry.clone(), live.projection_revision, false)));
        }
        Ok(row
            .deletions
            .values()
            .filter(|claim| claim.completed && claim.reference.digest == *digest)
            .max_by_key(|claim| claim.projection_revision)
            .map(|claim| (claim.entry.clone(), claim.projection_revision, true)))
    }
}
