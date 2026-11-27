use tracing::warn;
use trogon_sandbox_contract::{SnapshotAccountingRefusal, SnapshotAccountingResponse, SnapshotPublished};

use crate::quota::{QuotaStore, SnapshotQuota};
use crate::snapshot_live::LiveSnapshotStore;

pub async fn account_snapshot_published(
    live_store: &dyn LiveSnapshotStore,
    quota: &dyn QuotaStore,
    default_snapshot_quota: SnapshotQuota,
    event: &SnapshotPublished,
) -> SnapshotAccountingResponse {
    let confirmed = match quota.confirm_snapshot_publication(event, default_snapshot_quota).await {
        Ok(confirmed) => confirmed,
        Err(reason) => return SnapshotAccountingResponse::Refused { reason },
    };
    let Some(store) = quota.snapshot_store() else {
        return SnapshotAccountingResponse::Refused {
            reason: SnapshotAccountingRefusal::Backend,
        };
    };
    let projection = match store.projection(&event.tenant, &event.digest).await {
        Ok(projection) => projection,
        Err(reason) => return SnapshotAccountingResponse::Refused { reason },
    };
    if let Some((entry, revision, deleted)) = projection {
        let result = if deleted {
            live_store.remove_at(&event.digest, &entry, revision).await
        } else {
            live_store.put_at(&event.digest, &entry, revision).await
        };
        if let Err(error) = result {
            warn!(tenant = %event.tenant.as_str(), digest = %event.digest, %error, "snapshot accounting projection remains pending");
            return SnapshotAccountingResponse::Refused {
                reason: SnapshotAccountingRefusal::Backend,
            };
        }
    }
    confirmed.response
}
