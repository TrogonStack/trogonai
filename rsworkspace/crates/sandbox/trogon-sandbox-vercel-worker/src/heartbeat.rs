use std::collections::HashMap;
use std::time::SystemTime;

use trogon_sandbox_contract::{
    CapabilitySupport, ProviderKind, WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat,
    WorkerId,
};

use crate::{VercelPlan, profile_with_resources, resources};

/// What the worker tells the broker about itself: its capacity, what the plan lets it offer, and whether it takes
/// dispatches addressed to it alone.
pub fn build_heartbeat(
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
    regions: &[String],
    max_vcpus: u32,
    plan: VercelPlan,
    dispatch_protocol: WorkerDispatchProtocol,
) -> WorkerHeartbeat {
    let mut profile = profile_with_resources(resources(max_vcpus));
    profile.placement.regions = regions.to_vec();
    profile.lifecycle.max_lease_lifetime_seconds = Some(plan.max_session_seconds());
    WorkerHeartbeat {
        worker_id: worker_id.clone(),
        dispatch_protocol,
        native_volume_bindings: Vec::new(),
        provider: ProviderKind::Vercel,
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
