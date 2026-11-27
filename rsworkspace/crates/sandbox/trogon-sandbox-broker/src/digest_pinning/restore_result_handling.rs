use bytes::Bytes;
use tracing::info;
use trogon_sandbox_contract::{
    DigestMismatch, LeaseFailed, LeaseFailedReason, LeaseId, SnapshotRestoreResult, TenantId,
};
use trogon_sandbox_protocol::subjects::lease_failed;

use super::SnapshotPinStore;
use crate::bus::BrokerBus;
use crate::metrics::BrokerMetrics;

pub async fn handle_snapshot_restore_result(
    pin_store: &dyn SnapshotPinStore,
    bus: &dyn BrokerBus,
    metrics: Option<&BrokerMetrics>,
    lease_id: LeaseId,
    tenant: TenantId,
    result: SnapshotRestoreResult,
) -> anyhow::Result<()> {
    let expected = match pin_store.expected_digest(&lease_id).await? {
        Some(d) => d,
        None => return Ok(()),
    };

    if result.digest == expected {
        return Ok(());
    }

    if let Some(metrics) = metrics {
        metrics.digest_pin_mismatch_total.inc();
    }

    let failed = LeaseFailed {
        lease_id: lease_id.clone(),
        tenant,
        reason: LeaseFailedReason::DigestMismatch(DigestMismatch::new(expected, result.digest)?),
    };

    let subject = lease_failed(&lease_id);
    let payload = Bytes::from(serde_json::to_vec(&failed)?);
    bus.publish(&subject, payload).await?;

    info!(
        lease_id = %lease_id,
        "lease failed: snapshot digest mismatch"
    );

    pin_store.unpin(&lease_id).await
}
