use trogon_sandbox_contract::{Digest, TenantId};

use super::snapshot_removal_error::SnapshotRemovalError;
use crate::snapshot_accounting::{SnapshotAccountingStore, SnapshotDeletionClaim};
use crate::snapshot_catalog::ArtifactDeleter;
use crate::snapshot_live::LiveSnapshotStore;

pub async fn remove_accounted_snapshot(
    accounting: &SnapshotAccountingStore,
    live: &dyn LiveSnapshotStore,
    deleter: &dyn ArtifactDeleter,
    tenant: &TenantId,
    digest: &Digest,
) -> Result<Option<SnapshotDeletionClaim>, SnapshotRemovalError> {
    let claim = accounting
        .claim_delete(tenant, digest)
        .await
        .map_err(SnapshotRemovalError::ClaimRefused)?;
    let Some(claim) = claim else {
        if let Some((entry, revision, true)) = accounting
            .projection(tenant, digest)
            .await
            .map_err(SnapshotRemovalError::ReconciliationRefused)?
        {
            live.remove_at(digest, &entry, revision).await?;
        }
        if live.get_owned(tenant, digest).await?.is_some() {
            return Err(SnapshotRemovalError::LegacyOwnership);
        }
        return Ok(None);
    };
    if claim.reference.repository != trogon_sandbox_protocol::snapshot_repository_for_tenant(tenant) {
        return Err(SnapshotRemovalError::LegacyRepository);
    }
    deleter.delete(&claim.reference).await?;
    let completed = accounting
        .complete_delete(&claim)
        .await
        .map_err(SnapshotRemovalError::CompletionPending)?;
    live.remove_at(digest, &completed.entry, completed.projection_revision)
        .await?;
    Ok(Some(completed))
}
