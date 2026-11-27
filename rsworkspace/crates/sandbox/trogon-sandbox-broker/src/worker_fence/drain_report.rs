use trogon_sandbox_contract::{LeaseId, LeaseStatus, ProviderKind, WorkerId};

use super::{WorkerFenceStore, WorkerPlacementState};

/// Leases still held by one worker, for the operator watching it drain.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DrainReport {
    pub state: WorkerPlacementState,
    /// Leases the active-lease store attributes to the worker that are not
    /// yet released or failed, in lease-id order.
    pub remaining: Vec<(LeaseId, LeaseStatus)>,
}

impl DrainReport {
    pub fn is_drained(&self) -> bool {
        self.remaining.is_empty()
    }
}

/// Reports the fence state of `(provider, worker_id)` and the leases still on
/// it. Only leases dispatched to a named worker carry a worker id; a provider
/// that broadcasts dispatch leaves nothing to attribute, so its report is
/// always empty.
pub async fn drain_report(
    fences: &dyn WorkerFenceStore,
    active: &dyn crate::active_lease::ActiveLeaseStore,
    provider: ProviderKind,
    worker_id: &WorkerId,
) -> anyhow::Result<DrainReport> {
    let state = fences
        .get(provider, worker_id)
        .await?
        .map(|fence| fence.state)
        .unwrap_or_default();
    let mut remaining: Vec<_> = active
        .all()
        .await?
        .into_iter()
        .filter(|record| {
            record.provider == provider
                && record.worker_id.as_ref().is_some_and(|id| *id == *worker_id)
                && !matches!(record.status, LeaseStatus::Released | LeaseStatus::Failed)
        })
        .map(|record| (record.lease_id, record.status))
        .collect();
    remaining.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    Ok(DrainReport { state, remaining })
}
