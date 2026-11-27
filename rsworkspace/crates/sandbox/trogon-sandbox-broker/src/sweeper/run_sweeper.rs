use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::{LeaseReleased, LeaseReleasedReason};

use crate::active_lease::ActiveLeaseStore;
use crate::bus::BrokerBus;
use crate::idle::{IdleAction, IdlePauser, idle_capabilities, next_idle_action_with};
use crate::metrics::BrokerMetrics;
use crate::quota::QuotaStore;
use crate::usage_store::LeaseUsageStore;

/// Reclaims legacy provider leases whose deadline has passed or that sat idle
/// past their `idle_timeout_seconds`.
/// Targeted microVM attempts use BrokerCore's receipt-verified reconciliation.
///
/// Multiple broker replicas may sweep concurrently: the revision-checked
/// delete of the active-lease record is the claim, and only the replica that
/// wins it refunds quota.
#[allow(clippy::too_many_arguments)]
pub async fn run_sweeper(
    tick: Duration,
    active_leases: Arc<dyn ActiveLeaseStore>,
    quota: Arc<dyn QuotaStore>,
    bus: Arc<dyn BrokerBus>,
    metrics: BrokerMetrics,
    usage: Arc<dyn LeaseUsageStore>,
    pauser: Arc<dyn IdlePauser>,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("lease sweeper shutting down");
                return;
            }
            _ = interval.tick() => {
                sweep_once(&active_leases, &quota, &bus, &metrics, &usage, &pauser).await;
            }
        }
    }
}

pub(crate) async fn sweep_once(
    active_leases: &Arc<dyn ActiveLeaseStore>,
    quota: &Arc<dyn QuotaStore>,
    bus: &Arc<dyn BrokerBus>,
    metrics: &BrokerMetrics,
    usage: &Arc<dyn LeaseUsageStore>,
    pauser: &Arc<dyn IdlePauser>,
) {
    let records = match active_leases.all().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "sweeper: failed to list active leases");
            return;
        }
    };

    let now = SystemTime::now();

    for record in records {
        if record.dispatch_attempt.is_some() {
            continue;
        }
        let reason = if record.deadline < now {
            LeaseReleasedReason::Expired
        } else {
            let caps = idle_capabilities(&record, pauser.as_ref()).await;
            let Some(action) = next_idle_action_with(&record, now, caps) else {
                continue;
            };
            // Activity may have landed since the listing; re-read before acting.
            match active_leases.get(&record.lease_id).await {
                Ok(Some(fresh)) if next_idle_action_with(&fresh, now, caps) == Some(action) => {
                    match action {
                        // The pauser logs and backs off an action that fails.
                        IdleAction::Pause => {
                            pauser.pause_idle(&fresh).await;
                            continue;
                        }
                        IdleAction::Suspend => {
                            pauser.suspend_idle(&fresh).await;
                            continue;
                        }
                        IdleAction::Release => {}
                    }
                    LeaseReleasedReason::IdleTimeout
                }
                _ => continue,
            }
        };

        let lease_id = &record.lease_id;
        let tenant = &record.tenant;

        // Best-effort: ask the provider to stop the worker
        let cancel_subject = trogon_sandbox_protocol::subjects::provider_cancel(record.provider, lease_id);
        let _ = bus.publish(&cancel_subject, Bytes::new()).await;

        // Publish typed LeaseReleased event
        let payload = LeaseReleased {
            lease_id: lease_id.clone(),
            tenant: tenant.clone(),
            reason,
            snapshot_out: record.spec.snapshot_out().cloned(),
            worker_id: record.worker_id.clone(),
            correlation_id: record.correlation_id.clone(),
            restored_from: record.restored_from.clone(),
            released_at: Some(SystemTime::now()),
        };
        match serde_json::to_vec(&payload) {
            Ok(bytes) => {
                let subject = trogon_sandbox_protocol::subjects::lease_released(lease_id);
                if let Err(e) = bus.publish(&subject, Bytes::from(bytes)).await {
                    warn!(lease_id = %lease_id.as_str(), error = %e, "sweeper: failed to publish lease_released");
                }
            }
            Err(e) => {
                warn!(lease_id = %lease_id.as_str(), error = %e, "sweeper: failed to serialize LeaseReleased");
            }
        }

        // Claim the record first: only the replica whose delete takes effect
        // refunds, so concurrent sweepers cannot double-refund.
        match active_leases.delete(lease_id).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                warn!(lease_id = %lease_id.as_str(), error = %e, "sweeper: failed to claim active lease record");
                continue;
            }
        }

        // Freeze the final usage figures; best effort, the lease is already gone.
        if let Err(e) = usage.finish(&record, now).await {
            warn!(lease_id = %lease_id.as_str(), error = %e, "sweeper: failed to stamp final usage");
        }

        // Refund quota using the stored consumption amounts
        if let Err(e) = quota
            .release(tenant, record.cpu_seconds_consumed, record.wall_seconds_consumed)
            .await
        {
            warn!(lease_id = %lease_id.as_str(), error = %e, "sweeper: failed to release quota");
        }

        if let Err(e) = quota.decrement_active_leases(tenant).await {
            warn!(lease_id = %lease_id.as_str(), error = %e, "sweeper: failed to decrement active lease count");
        }

        metrics.dec_active_leases(tenant.as_str());
        info!(lease_id = %lease_id.as_str(), tenant = %tenant.as_str(), reason = ?reason, "sweeper: reclaimed lease");
    }
}
