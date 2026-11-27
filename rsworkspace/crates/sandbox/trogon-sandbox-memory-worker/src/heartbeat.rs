use std::time::SystemTime;

use trogon_sandbox_contract::{ProviderKind, WorkerCapabilities, WorkerCapacity, WorkerHeartbeat, WorkerId};

use crate::PROFILE;

/// A heartbeat declaring this worker's honest profile.
pub fn build_heartbeat(
    kind: ProviderKind,
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
) -> WorkerHeartbeat {
    build_heartbeat_in_regions(kind, worker_id, max_concurrent_leases, current_leases, &[])
}

/// A heartbeat for a worker that claims to place sandboxes in `regions`.
pub fn build_heartbeat_in_regions(
    kind: ProviderKind,
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
    regions: &[String],
) -> WorkerHeartbeat {
    let mut profile = PROFILE.clone();
    profile.provider = kind;
    profile.placement.regions = regions.to_vec();
    WorkerHeartbeat {
        supports_lease_epoch: Default::default(),
        worker_id: worker_id.clone(),
        provider: kind,
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
