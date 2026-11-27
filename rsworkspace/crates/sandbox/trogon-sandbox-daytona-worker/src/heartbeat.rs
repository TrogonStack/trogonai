use std::collections::HashMap;
use std::time::SystemTime;

use trogon_sandbox_contract::{
    CapabilitySupport, ProviderKind, WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat,
    WorkerId,
};

use crate::PROFILE;

/// What the worker tells the broker about itself: its capacity, the regions it offers and what Daytona lets it offer.
pub fn build_heartbeat(
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
    regions: &[String],
) -> WorkerHeartbeat {
    let mut profile = PROFILE.clone();
    profile.placement.regions = regions.to_vec();
    WorkerHeartbeat {
        worker_id: worker_id.clone(),
        dispatch_protocol: WorkerDispatchProtocol::Legacy,
        native_volume_bindings: Vec::new(),
        provider: ProviderKind::Daytona,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile,
        warm_pool_population: HashMap::new(),
        supports_lease_epoch: CapabilitySupport::Supported,
    }
}

#[cfg(test)]
mod tests;
