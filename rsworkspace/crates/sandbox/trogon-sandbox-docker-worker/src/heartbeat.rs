use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{
    CapabilitySupport, Digest, ProviderKind, ResourceCapability, WorkerCapabilities, WorkerCapacity,
    WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::DEFAULT_HEARTBEAT_INTERVAL;

use crate::adapter::DockerWorkerProvider;
use crate::browser;

use constants::WARM_POOL_SWEEP_INTERVAL;

mod constants;

/// Sweeps idle warm containers until `cancel` fires.
pub fn spawn_warm_pool_sweeper(provider: Arc<DockerWorkerProvider>, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(WARM_POOL_SWEEP_INTERVAL);
        loop {
            tokio::select! {
                _ = ticker.tick() => provider.sweep_warm_pool().await,
                _ = cancel.cancelled() => break,
            }
        }
    });
}

/// Publishes this worker's heartbeat on its subject until `cancel` fires, starting once intake is open.
pub fn spawn_heartbeat_task(
    client: async_nats::Client,
    worker_id: WorkerId,
    machine_resources: ResourceCapability,
    max_concurrent_leases: u32,
    current_leases: Arc<AtomicU32>,
    provider: Arc<DockerWorkerProvider>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = provider.wait_for_intake() => {}
        }
        let subject = trogon_sandbox_protocol::subjects::worker_heartbeat(ProviderKind::LocalDocker, &worker_id);
        let mut ticker = tokio::time::interval(DEFAULT_HEARTBEAT_INTERVAL);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {}
            }
            let mut heartbeat = build_heartbeat(
                &worker_id,
                machine_resources,
                max_concurrent_leases,
                current_leases.load(Ordering::Relaxed),
                provider.cached_digests(),
                provider.warm_pool_population().await,
                provider.browser_support(),
            );
            if provider.dispatch_ledger().is_some() {
                heartbeat.profile.snapshot.supports_in_place_rollback = CapabilitySupport::Unsupported;
            }
            if provider.targeted_intake_ready().await {
                heartbeat.dispatch_protocol = WorkerDispatchProtocol::TargetedV1;
                heartbeat.supports_lease_epoch = CapabilitySupport::Supported;
            }
            match serde_json::to_vec(&heartbeat) {
                Ok(payload) => {
                    if let Err(error) = client.publish(subject.clone(), payload.into()).await {
                        warn!(%error, "failed to publish Docker heartbeat");
                    }
                }
                Err(error) => warn!(%error, "failed to serialize Docker heartbeat"),
            }
        }
    });
}

/// The heartbeat this worker publishes.
pub fn build_heartbeat(
    worker_id: &WorkerId,
    machine_resources: ResourceCapability,
    max_concurrent_leases: u32,
    current_leases: u32,
    cached_digests: Vec<Digest>,
    warm_pool_population: HashMap<Digest, u32>,
    browser_support: Option<&browser::BrowserSupport>,
) -> WorkerHeartbeat {
    let profile = browser::profile_with_browser(machine_resources, browser_support);
    WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::LocalDocker,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests,
        profile,
        warm_pool_population,
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    }
}

#[cfg(test)]
mod tests;
