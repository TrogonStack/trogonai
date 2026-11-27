use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{
    CapabilityRequirements, CapabilitySupport, Digest, ProviderCapabilityProfile, ProviderKind, WorkerCapabilities,
    WorkerHeartbeat, WorkerId,
};

use crate::constants::ALWAYS_AVAILABLE_WORKER;
use crate::default_capability_profile::default_capability_profile;
use crate::worker_fence::FenceView;

use super::WorkerRegistry;
use super::heartbeat_selection::*;
use super::requires_targeted_dispatch;

pub struct InMemoryWorkerRegistry {
    heartbeats: RwLock<HashMap<(ProviderKind, WorkerId), WorkerHeartbeat>>,
    heartbeat_ttl: Duration,
    always_available: bool,
    fences: FenceView,
}

impl InMemoryWorkerRegistry {
    pub fn new(heartbeat_ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            heartbeats: RwLock::new(HashMap::new()),
            heartbeat_ttl,
            always_available: false,
            fences: FenceView::new(),
        })
    }

    /// Construct a registry that always reports workers as available.
    /// Use this when `enforce_worker_registry = false` so the call site
    /// in `BrokerCore` remains uniform.
    pub fn with_always_available(heartbeat_ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            heartbeats: RwLock::new(HashMap::new()),
            heartbeat_ttl,
            always_available: true,
            fences: FenceView::new(),
        })
    }

    /// The fences this registry honours; refresh it from a
    /// [`crate::worker_fence::WorkerFenceStore`] to cordon workers.
    pub fn fences(&self) -> FenceView {
        self.fences.clone()
    }

    pub async fn inject(&self, hb: WorkerHeartbeat) {
        self.heartbeats
            .write()
            .await
            .insert((hb.provider, hb.worker_id.clone()), hb);
    }
}

#[async_trait]
impl WorkerRegistry for InMemoryWorkerRegistry {
    async fn current_worker(
        &self,
        provider: ProviderKind,
        worker: &trogon_sandbox_contract::WorkerId,
    ) -> Option<WorkerHeartbeat> {
        let heartbeats = self.heartbeats.read().await;
        let heartbeat = heartbeats.get(&(provider, worker.clone()))?;
        SystemTime::now()
            .duration_since(heartbeat.emitted_at)
            .is_ok_and(|age| age <= self.heartbeat_ttl)
            .then(|| heartbeat.clone())
    }

    async fn available_for(&self, provider: ProviderKind) -> Vec<WorkerId> {
        self.eligible_for(provider, None)
            .await
            .into_iter()
            .map(|hb| hb.worker_id)
            .collect()
    }

    async fn any_available(&self, provider: ProviderKind) -> bool {
        if self.always_available {
            return true;
        }
        self.any_eligible(provider, None).await
    }

    async fn eligible_for(
        &self,
        provider: ProviderKind,
        requirements: Option<&CapabilityRequirements>,
    ) -> Vec<WorkerHeartbeat> {
        if self.always_available {
            return WorkerId::new(ALWAYS_AVAILABLE_WORKER)
                .into_iter()
                .map(|worker_id| WorkerHeartbeat {
                    supports_lease_epoch: CapabilitySupport::Unsupported,
                    worker_id,
                    provider,
                    capacity: trogon_sandbox_contract::WorkerCapacity {
                        max_concurrent_leases: 1,
                        current_leases: 0,
                        capabilities: WorkerCapabilities::from(&default_capability_profile()),
                    },
                    emitted_at: SystemTime::now(),
                    cached_digests: vec![],
                    profile: default_capability_profile(),
                    warm_pool_population: HashMap::new(),
                    native_volume_bindings: Default::default(),
                    dispatch_protocol: if requires_targeted_dispatch(provider) {
                        trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1
                    } else {
                        Default::default()
                    },
                })
                .collect();
        }
        let now = SystemTime::now();
        self.heartbeats
            .read()
            .await
            .values()
            .filter(|hb| {
                self.fences.admits_placements(hb.provider, &hb.worker_id)
                    && heartbeat_eligible(hb, provider, now, self.heartbeat_ttl, requirements)
            })
            .cloned()
            .collect()
    }

    async fn any_eligible(&self, provider: ProviderKind, requirements: Option<&CapabilityRequirements>) -> bool {
        if self.always_available {
            return true;
        }
        !self.eligible_for(provider, requirements).await.is_empty()
    }

    async fn any_eligible_with_cached_digest(
        &self,
        provider: ProviderKind,
        requirements: Option<&CapabilityRequirements>,
        digest: &Digest,
    ) -> bool {
        if self.always_available {
            return true;
        }
        let now = SystemTime::now();
        self.heartbeats.read().await.values().any(|hb| {
            self.fences.admits_placements(hb.provider, &hb.worker_id)
                && heartbeat_eligible(hb, provider, now, self.heartbeat_ttl, requirements)
                && heartbeat_has_cached_digest(hb, digest)
        })
    }

    async fn any_can_restore(&self, provider: ProviderKind, requirements: Option<&CapabilityRequirements>) -> bool {
        if self.always_available {
            return true;
        }
        let now = SystemTime::now();
        self.heartbeats.read().await.values().any(|hb| {
            self.fences.admits_placements(hb.provider, &hb.worker_id)
                && heartbeat_can_restore(hb, provider, now, self.heartbeat_ttl, requirements)
        })
    }

    async fn profile_for_provider(&self, provider: ProviderKind) -> Option<ProviderCapabilityProfile> {
        let now = SystemTime::now();
        self.heartbeats
            .read()
            .await
            .values()
            .filter(|hb| {
                hb.provider == provider
                    && now
                        .duration_since(hb.emitted_at)
                        .map(|d| d <= self.heartbeat_ttl)
                        .unwrap_or(false)
            })
            .max_by_key(|hb| hb.emitted_at)
            .map(|hb| hb.profile.clone())
    }

    async fn warm_pool_count(&self, provider: ProviderKind, digest: &Digest) -> u32 {
        if self.always_available {
            return 0;
        }
        let now = SystemTime::now();
        self.heartbeats
            .read()
            .await
            .values()
            .filter(|hb| {
                hb.provider == provider
                    && now
                        .duration_since(hb.emitted_at)
                        .map(|d| d <= self.heartbeat_ttl)
                        .unwrap_or(false)
            })
            .map(|hb| hb.warm_pool_population.get(digest).copied().unwrap_or(0))
            .sum()
    }

    async fn list_capability_profiles(&self) -> Vec<ProviderCapabilityProfile> {
        if self.always_available {
            return vec![default_capability_profile()];
        }
        let now = SystemTime::now();
        let heartbeats = self.heartbeats.read().await;
        freshest_profiles_by_provider(&heartbeats, now, self.heartbeat_ttl)
    }
}
