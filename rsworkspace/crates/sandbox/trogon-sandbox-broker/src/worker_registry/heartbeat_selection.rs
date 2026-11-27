use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::{
    CapabilityRequirements, Digest, ProviderCapabilityProfile, ProviderKind, WorkerHeartbeat, WorkerId,
};

use crate::profile_satisfaction::ProfileSatisfies;

pub(super) fn heartbeat_eligible(
    hb: &WorkerHeartbeat,
    provider: ProviderKind,
    now: SystemTime,
    heartbeat_ttl: Duration,
    requirements: Option<&CapabilityRequirements>,
) -> bool {
    hb.provider == provider
        && hb.capacity.current_leases < hb.capacity.max_concurrent_leases
        && now
            .duration_since(hb.emitted_at)
            .map(|d| d <= heartbeat_ttl)
            .unwrap_or(false)
        && requirements.is_none_or(|r| hb.profile.satisfies(r))
}

pub(super) fn heartbeat_has_cached_digest(hb: &WorkerHeartbeat, digest: &Digest) -> bool {
    hb.cached_digests.iter().any(|d| d == digest)
}

pub(super) fn heartbeat_can_restore(
    hb: &WorkerHeartbeat,
    provider: ProviderKind,
    now: SystemTime,
    heartbeat_ttl: Duration,
    requirements: Option<&CapabilityRequirements>,
) -> bool {
    heartbeat_eligible(hb, provider, now, heartbeat_ttl, requirements) && hb.profile.snapshot.can_restore.is_supported()
}

/// Groups fresh heartbeats by provider and keeps only the most recently
/// emitted heartbeat's profile per provider.
pub(super) fn freshest_profiles_by_provider(
    heartbeats: &HashMap<(ProviderKind, WorkerId), WorkerHeartbeat>,
    now: SystemTime,
    heartbeat_ttl: Duration,
) -> Vec<ProviderCapabilityProfile> {
    let mut freshest: HashMap<ProviderKind, &WorkerHeartbeat> = HashMap::new();
    for hb in heartbeats.values() {
        let fresh = now
            .duration_since(hb.emitted_at)
            .map(|d| d <= heartbeat_ttl)
            .unwrap_or(false);
        if !fresh {
            continue;
        }
        freshest
            .entry(hb.provider)
            .and_modify(|current| {
                if hb.emitted_at > current.emitted_at {
                    *current = hb;
                }
            })
            .or_insert(hb);
    }
    freshest.into_values().map(|hb| hb.profile.clone()).collect()
}

// ── InMemoryWorkerRegistry ────────────────────────────────────────────────────
