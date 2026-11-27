use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use trogon_sandbox_contract::{
    CapabilityRequirements, Digest, ProviderCapabilityProfile, ProviderKind, WorkerHeartbeat, WorkerId,
};

use crate::worker_fence::FenceView;

use super::WorkerRegistry;
use super::heartbeat_selection::*;

pub struct NatsWorkerRegistry {
    heartbeats: Arc<RwLock<HashMap<(ProviderKind, WorkerId), WorkerHeartbeat>>>,
    heartbeat_ttl: Duration,
    fences: FenceView,
}

impl NatsWorkerRegistry {
    /// The fences this registry honours; refresh it from a
    /// [`crate::worker_fence::WorkerFenceStore`] to cordon workers.
    pub fn fences(&self) -> FenceView {
        self.fences.clone()
    }

    pub async fn new(
        client: async_nats::Client,
        heartbeat_ttl: Duration,
        cancel: CancellationToken,
    ) -> anyhow::Result<Arc<Self>> {
        let heartbeats: Arc<RwLock<HashMap<(ProviderKind, WorkerId), WorkerHeartbeat>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let registry = Arc::new(Self {
            heartbeats: Arc::clone(&heartbeats),
            heartbeat_ttl,
            fences: FenceView::new(),
        });

        // Subscribe to all worker heartbeats across every provider.
        let mut sub = client.subscribe("sandbox.worker.heartbeat.*.*").await?;

        // Subscription task: receives and indexes heartbeats.
        {
            let heartbeats = Arc::clone(&heartbeats);
            let cancel = cancel.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        maybe_msg = sub.next() => {
                            match maybe_msg {
                                None => break,
                                Some(msg) => {
                                    match serde_json::from_slice::<WorkerHeartbeat>(&msg.payload) {
                                        Ok(hb) => {
                                            debug!(
                                                worker_id = %hb.worker_id,
                                                provider = ?hb.provider,
                                                "heartbeat received"
                                            );
                                            heartbeats
                                                .write()
                                                .await
                                                .insert((hb.provider, hb.worker_id.clone()), hb);
                                        }
                                        Err(e) => {
                                            warn!(error = %e, "failed to decode worker heartbeat");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }

        // Sweeper task: evicts stale entries every `ttl / 3`.
        {
            let heartbeats = Arc::clone(&heartbeats);
            let ttl = heartbeat_ttl;
            let sweep_interval = ttl / 3;
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(sweep_interval);
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = ticker.tick() => {
                            let now = SystemTime::now();
                            heartbeats.write().await.retain(|_, hb| {
                                now.duration_since(hb.emitted_at)
                                    .map(|d| d <= ttl)
                                    .unwrap_or(false)
                            });
                        }
                    }
                }
            });
        }

        Ok(registry)
    }
}

#[async_trait]
impl WorkerRegistry for NatsWorkerRegistry {
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
        self.any_eligible(provider, None).await
    }

    async fn eligible_for(
        &self,
        provider: ProviderKind,
        requirements: Option<&CapabilityRequirements>,
    ) -> Vec<WorkerHeartbeat> {
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
        !self.eligible_for(provider, requirements).await.is_empty()
    }

    async fn any_eligible_with_cached_digest(
        &self,
        provider: ProviderKind,
        requirements: Option<&CapabilityRequirements>,
        digest: &Digest,
    ) -> bool {
        let now = SystemTime::now();
        self.heartbeats.read().await.values().any(|hb| {
            self.fences.admits_placements(hb.provider, &hb.worker_id)
                && heartbeat_eligible(hb, provider, now, self.heartbeat_ttl, requirements)
                && heartbeat_has_cached_digest(hb, digest)
        })
    }

    async fn any_can_restore(&self, provider: ProviderKind, requirements: Option<&CapabilityRequirements>) -> bool {
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
        let now = SystemTime::now();
        let heartbeats = self.heartbeats.read().await;
        freshest_profiles_by_provider(&heartbeats, now, self.heartbeat_ttl)
    }
}
