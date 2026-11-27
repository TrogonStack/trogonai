use serde::Serialize;
use trogon_sandbox_contract::{CapabilitySupport, Digest, IsolationTier, ProviderKind, WorkerHeartbeat, WorkerId};

use crate::unix_seconds::unix_seconds;

#[derive(Debug, Serialize)]
struct CapabilitySummary<'a> {
    can_snapshot: CapabilitySupport,
    can_restore: CapabilitySupport,
    can_checkpoint_running: CapabilitySupport,
    supports_warm_pool: CapabilitySupport,
    supports_named_tags: CapabilitySupport,
    isolation_tiers: &'a [IsolationTier],
}

/// One worker heartbeat on the dashboard stream.
#[derive(Debug, Serialize)]
pub(crate) struct WorkerRow<'a> {
    provider: ProviderKind,
    worker_id: &'a WorkerId,
    capability_summary: CapabilitySummary<'a>,
    cached_digests_count: usize,
    cached_digests: Vec<&'a str>,
    last_heartbeat: u64,
    current_leases: u32,
    max_concurrent_leases: u32,
}

impl<'a> From<&'a WorkerHeartbeat> for WorkerRow<'a> {
    fn from(heartbeat: &'a WorkerHeartbeat) -> Self {
        let profile = &heartbeat.profile;
        Self {
            provider: heartbeat.provider,
            worker_id: &heartbeat.worker_id,
            capability_summary: CapabilitySummary {
                can_snapshot: profile.snapshot.can_snapshot,
                can_restore: profile.snapshot.can_restore,
                can_checkpoint_running: profile.snapshot.can_checkpoint_running,
                supports_warm_pool: profile.lifecycle.supports_warm_pool,
                supports_named_tags: profile.lifecycle.supports_named_tags,
                isolation_tiers: &profile.isolation.supported_tiers,
            },
            cached_digests_count: heartbeat.cached_digests.len(),
            cached_digests: heartbeat.cached_digests.iter().map(Digest::as_str).collect(),
            last_heartbeat: unix_seconds(heartbeat.emitted_at),
            current_leases: heartbeat.capacity.current_leases,
            max_concurrent_leases: heartbeat.capacity.max_concurrent_leases,
        }
    }
}
