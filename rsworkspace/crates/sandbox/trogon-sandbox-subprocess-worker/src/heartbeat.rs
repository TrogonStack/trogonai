use std::sync::Arc;
use std::time::SystemTime;

use async_nats::Client;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    CapabilitySupport, ProviderCapabilityProfile, ProviderKind, WorkerCapabilities, WorkerCapacity, WorkerHeartbeat,
    WorkerId,
};
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, spawn_heartbeat_loop};

pub fn build_heartbeat(
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
    profile: &ProviderCapabilityProfile,
) -> WorkerHeartbeat {
    WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::Subprocess,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile: profile.clone(),
        warm_pool_population: Default::default(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    }
}

/// Tells the broker this worker exists and which capabilities it advertises.
pub fn spawn_heartbeat_task(
    client: Client,
    worker_id: WorkerId,
    max_concurrent_leases: u32,
    current_leases: Arc<dyn Fn() -> u32 + Send + Sync>,
    profile: ProviderCapabilityProfile,
    cancel: CancellationToken,
) {
    let id = worker_id.clone();
    spawn_heartbeat_loop(
        client,
        ProviderKind::Subprocess,
        worker_id,
        DEFAULT_HEARTBEAT_INTERVAL,
        cancel,
        move || {
            let heartbeat = build_heartbeat(&id, max_concurrent_leases, current_leases(), &profile);
            async move { heartbeat }
        },
    );
}

#[cfg(test)]
mod tests;
