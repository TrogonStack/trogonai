use tracing::warn;
use trogon_sandbox_contract::{SnapshotAccountingResponse, SnapshotPublished};

use super::account_snapshot_published::account_snapshot_published;
use crate::quota::{QuotaStore, SnapshotQuota};
use crate::snapshot_live::LiveSnapshotStore;

pub async fn handle_snapshot_published(
    live_store: &dyn LiveSnapshotStore,
    quota: &dyn QuotaStore,
    default_snapshot_quota: SnapshotQuota,
    event: SnapshotPublished,
) {
    let response = account_snapshot_published(live_store, quota, default_snapshot_quota, &event).await;
    if let SnapshotAccountingResponse::Refused { reason } = response {
        warn!(tenant = %event.tenant.as_str(), digest = %event.digest, ?reason, "snapshot accounting was not confirmed");
    }
}
