use tracing::warn;
use trogon_sandbox_contract::SnapshotDeleted;

use crate::quota::QuotaStore;
use crate::snapshot_live::LiveSnapshotStore;

pub async fn handle_snapshot_deleted(
    live_store: &dyn LiveSnapshotStore,
    quota: &dyn QuotaStore,
    event: SnapshotDeleted,
) {
    let Some(store) = quota.snapshot_store() else {
        return;
    };
    match store.deleted_receipt(&event).await {
        Ok(claim) => {
            if let Err(error) = live_store
                .remove_at(&event.digest, &claim.entry, claim.projection_revision)
                .await
            {
                warn!(tenant = %event.tenant.as_str(), digest = %event.digest, %error, "deleted snapshot projection remains pending");
            }
        }
        Err(reason) => {
            warn!(tenant = %event.tenant.as_str(), digest = %event.digest, ?reason, "snapshot deletion lacks a completed host claim")
        }
    }
}
