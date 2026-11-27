use std::collections::HashMap;
use std::time::SystemTime;

use trogon_sandbox_contract::{
    CapabilitySupport, Digest, ProviderCapabilityProfile, ProviderKind, WorkerCapabilities, WorkerCapacity,
    WorkerHeartbeat, WorkerId,
};

/// The heartbeat this worker publishes, whatever its load.
pub fn build_heartbeat(
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    profile: &ProviderCapabilityProfile,
    current_leases: u32,
    cached_digests: Vec<Digest>,
    warm_pool_population: HashMap<Digest, u32>,
) -> WorkerHeartbeat {
    WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::K8sMicroVm,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests,
        profile: profile.clone(),
        warm_pool_population,
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    }
}
