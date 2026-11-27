use std::time::SystemTime;

use async_nats::jetstream;
use trogon_sandbox_broker::active_lease::{ActiveLeaseStore, NatsKvActiveLeaseStore};
use trogon_sandbox_broker::worker_fence::{
    NatsKvWorkerFenceStore, WorkerFence, WorkerFenceStore, WorkerPlacementState, drain_report,
};
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::subjects::provider_token;

use crate::{AdminError, Console};

async fn open_fences(js: jetstream::Context) -> Result<NatsKvWorkerFenceStore, AdminError> {
    NatsKvWorkerFenceStore::new(js)
        .await
        .map_err(AdminError::step("open worker-fence store"))
}

async fn open_active(js: jetstream::Context, bucket: &str) -> Result<NatsKvActiveLeaseStore, AdminError> {
    NatsKvActiveLeaseStore::new(js, bucket)
        .await
        .map_err(AdminError::step("open active-lease store"))
}

/// Sets the placement state of one worker and prints the outcome.
pub async fn set_state(
    js: jetstream::Context,
    active_lease_bucket: &str,
    provider: ProviderKind,
    worker_id: &WorkerId,
    state: WorkerPlacementState,
    reason: Option<String>,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let fences = open_fences(js.clone()).await?;
    fences
        .set(&WorkerFence {
            provider,
            worker_id: worker_id.clone(),
            state,
            reason,
            updated_at: SystemTime::now(),
        })
        .await
        .map_err(AdminError::step("set worker fence"))?;
    console.out(format_args!(
        "{} {worker_id}: {state} (brokers pick this up within a few seconds)",
        provider_token(provider)
    ))?;
    if state == WorkerPlacementState::Draining {
        let active = open_active(js, active_lease_bucket).await?;
        print_status(&fences, &active, provider, worker_id, console).await?;
    }
    Ok(())
}

/// Prints the fence state of one worker and the leases still on it.
pub async fn status(
    js: jetstream::Context,
    active_lease_bucket: &str,
    provider: ProviderKind,
    worker_id: &WorkerId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let fences = open_fences(js.clone()).await?;
    let active = open_active(js, active_lease_bucket).await?;
    print_status(&fences, &active, provider, worker_id, console).await
}

async fn print_status(
    fences: &dyn WorkerFenceStore,
    active: &dyn ActiveLeaseStore,
    provider: ProviderKind,
    worker_id: &WorkerId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let report = drain_report(fences, active, provider, worker_id)
        .await
        .map_err(AdminError::step("build drain report"))?;
    console.out(format_args!("state: {}", report.state))?;
    console.out(format_args!("remaining leases: {}", report.remaining.len()))?;
    for (lease_id, status) in &report.remaining {
        console.out(format_args!("  {lease_id} {status:?}"))?;
    }
    if report.state == WorkerPlacementState::Draining && report.is_drained() {
        console.out("drained: no live leases are attributed to this worker")?;
    }
    Ok(())
}

/// Lists every fenced worker.
pub async fn list(js: jetstream::Context, console: &mut Console<'_>) -> Result<(), AdminError> {
    let mut all = open_fences(js)
        .await?
        .list()
        .await
        .map_err(AdminError::step("list worker fences"))?;
    if all.is_empty() {
        return console.out("no fenced workers");
    }
    all.sort_by(|a, b| {
        (provider_token(a.provider), a.worker_id.as_str()).cmp(&(provider_token(b.provider), b.worker_id.as_str()))
    });
    for fence in all {
        let reason = fence.reason.map(|reason| format!(" ({reason})")).unwrap_or_default();
        console.out(format_args!(
            "{} {} {}{reason}",
            provider_token(fence.provider),
            fence.worker_id,
            fence.state
        ))?;
    }
    Ok(())
}
