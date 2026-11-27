use std::collections::HashMap;
use std::time::SystemTime;

use trogon_sandbox_contract::{
    CapabilitySupport, ProviderKind, WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat,
    WorkerId,
};

use crate::profile::PROFILE;

/// What the worker tells the broker about itself: its capacity, the regions it offers and how it takes assignments.
pub fn build_heartbeat(
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
    regions: &[String],
    dispatch_protocol: WorkerDispatchProtocol,
) -> WorkerHeartbeat {
    let mut profile = PROFILE.clone();
    profile.placement.regions = regions.to_vec();
    WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::E2b,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile,
        warm_pool_population: HashMap::new(),
        dispatch_protocol,
        native_volume_bindings: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
