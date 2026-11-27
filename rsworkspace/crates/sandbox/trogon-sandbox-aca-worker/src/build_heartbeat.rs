use std::time::SystemTime;

use trogon_sandbox_contract::{
    CapabilitySupport, ProviderKind, WorkerCapabilities, WorkerCapacity, WorkerHeartbeat, WorkerId,
};

use crate::PROFILE;

/// The heartbeat this worker publishes, advertising `region` when it has one.
pub fn build_heartbeat(
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
    region: Option<&str>,
) -> WorkerHeartbeat {
    let mut profile = PROFILE.clone();
    profile.placement.regions = region.map(str::to_owned).into_iter().collect();
    WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::AzureContainerApps,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile,
        warm_pool_population: Default::default(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    }
}
