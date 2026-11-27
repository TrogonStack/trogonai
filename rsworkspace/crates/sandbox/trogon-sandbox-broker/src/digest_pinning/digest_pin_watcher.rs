use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{LeaseId, SnapshotRestoreResult, TenantId};
use trogon_sandbox_protocol::subjects::snapshot_result_evidence_filter;

use super::{SnapshotPinStore, handle_snapshot_restore_result};
use crate::bus::BrokerBus;
use crate::metrics::BrokerMetrics;

pub(super) fn lease_id_from_snapshot_result_subject(subject: &str) -> Option<LeaseId> {
    let rest = subject.strip_prefix("sandbox.evidence.")?;
    let lease_id = rest.strip_suffix(".snapshot-result")?;
    if lease_id.is_empty() {
        return None;
    }
    Some(LeaseId::new(lease_id))
}

pub async fn run_digest_pin_watcher(
    client: async_nats::Client,
    pin_store: Arc<dyn SnapshotPinStore>,
    bus: Arc<dyn BrokerBus>,
    metrics: BrokerMetrics,
    lease_tenants: Arc<RwLock<HashMap<LeaseId, TenantId>>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let mut sub = client.subscribe(snapshot_result_evidence_filter()).await?;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            maybe_msg = sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let lease_id = match lease_id_from_snapshot_result_subject(msg.subject.as_str()) {
                    Some(id) => id,
                    None => continue,
                };

                let result: SnapshotRestoreResult = match serde_json::from_slice(&msg.payload) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(lease_id = %lease_id, error = %e, "failed to decode snapshot restore result");
                        continue;
                    }
                };

                let Some(tenant) = lease_tenants.read().await.get(&lease_id).cloned() else {
                    warn!(lease_id = %lease_id, "snapshot restore result for a lease with no known tenant");
                    continue;
                };

                if let Err(e) = handle_snapshot_restore_result(
                    pin_store.as_ref(),
                    bus.as_ref(),
                    Some(&metrics),
                    lease_id,
                    tenant,
                    result,
                )
                .await
                {
                    warn!(error = %e, "digest pinning handler failed");
                }
            }
        }
    }

    Ok(())
}
