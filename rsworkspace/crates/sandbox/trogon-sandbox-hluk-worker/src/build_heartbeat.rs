use std::time::SystemTime;

use trogon_sandbox_contract::{
    CapabilitySupport, ProviderKind, WorkerCapabilities, WorkerCapacity, WorkerHeartbeat, WorkerId,
};

use crate::PROFILE;

/// The heartbeat this worker publishes while it holds `current_leases`.
pub fn build_heartbeat(worker_id: &WorkerId, max_concurrent_leases: u32, current_leases: u32) -> WorkerHeartbeat {
    WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::Hluk,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::empty(),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile: PROFILE.clone(),
        warm_pool_population: Default::default(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    }
}
