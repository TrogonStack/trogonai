use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use trogon_sandbox_contract::{Digest, NetworkPolicy, OciArtifactRef};
use trogon_sandbox_oci::OciClient;
use trogon_sandbox_providers::{
    ANNOTATION_DIGEST, K8sMicroVmProvider, K8sResourceKind, K8sResourceReceipt, LABEL_WARM_POOL, WARM_POOL_IDLE,
    pod_age_seconds,
};

use crate::constants::WARM_POOL_RECONCILE_INTERVAL;
use crate::slot_census::SlotCensus;
use crate::snapshot::pull_rootfs;
use crate::warm_pool_planning::{
    new_warm_restore_secret_name, pick_source, plan_refill, recovered_source, warm_pod_fits,
};
use crate::{DigestCache, WarmPodClaim, WarmPoolConfig, WarmPoolError, WarmSlotState};

pub struct WarmPool {
    inner: Arc<K8sMicroVmProvider>,
    oci: Arc<OciClient>,
    digest_cache: Arc<DigestCache>,
    config: WarmPoolConfig,
    default_image: String,
    snapshot_registry: String,
    snapshot_repository: String,
    runtime_class: String,
    warm_cpu: f32,
    warm_memory_mib: u32,
    population: Mutex<HashMap<Digest, u32>>,
    /// Idle pods that exist but are not running yet, per digest. They count
    /// toward the pool's size when deciding what to create, so a slow start
    /// is not mistaken for a shortfall.
    warming: Mutex<HashMap<Digest, u32>>,
    pending_cleanup: Mutex<Vec<Vec<K8sResourceReceipt>>>,
    /// Where each digest was last seen, so a warm pod is built from the
    /// artifact a lease actually restored or a checkpoint actually pushed. The
    /// broker names a repository per tenant, which the worker's configured
    /// repository does not match.
    sources: std::sync::Mutex<HashMap<Digest, OciArtifactRef>>,
}

impl WarmPool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inner: Arc<K8sMicroVmProvider>,
        oci: Arc<OciClient>,
        digest_cache: Arc<DigestCache>,
        config: WarmPoolConfig,
        default_image: String,
        snapshot_registry: String,
        snapshot_repository: String,
        runtime_class: String,
    ) -> Self {
        Self {
            inner,
            oci,
            digest_cache,
            config,
            default_image,
            snapshot_registry,
            snapshot_repository,
            runtime_class,
            warm_cpu: 0.25,
            warm_memory_mib: 256,
            population: Mutex::new(HashMap::new()),
            warming: Mutex::new(HashMap::new()),
            pending_cleanup: Mutex::new(Vec::new()),
            sources: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Records where `reference`'s digest can be pulled from.
    pub fn remember_source(&self, reference: &OciArtifactRef) {
        self.sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(reference.digest.clone(), reference.clone());
    }

    /// The artifact to pull for `digest`: where it was last seen, or the
    /// worker's configured registry and repository when it was never seen.
    pub fn source_for(&self, digest: &Digest) -> OciArtifactRef {
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        pick_source(
            &mut sources,
            &self.digest_cache.list(),
            digest,
            &self.snapshot_registry,
            &self.snapshot_repository,
        )
    }

    /// Whether a warm pod is big enough for a lease asking for `vcpu_millis`
    /// and `memory_mib`. Warm pods are created before any lease names a size,
    /// so a larger request must cold start rather than run undersized.
    pub fn fits(&self, vcpu_millis: Option<u32>, memory_mib: u32) -> bool {
        warm_pod_fits(self.warm_cpu, self.warm_memory_mib, vcpu_millis, memory_mib)
    }

    pub async fn population(&self) -> HashMap<Digest, u32> {
        self.population.lock().await.clone()
    }

    /// Claims an idle pod for `digest` and applies `policy` to it before
    /// returning. A warm pod is created with no `NetworkPolicy`, so without
    /// this step the lease would run with open egress. If the policy cannot be
    /// applied the pod is deleted rather than handed out, and the claim fails.
    ///
    /// `timeout` is the lease's lifetime, stamped on the pod so the lease
    /// sweeper reclaims it once the lease runs out; zero means no deadline.
    pub async fn try_claim(
        &self,
        digest: &Digest,
        lease_id: &str,
        tenant_id: &str,
        policy: &NetworkPolicy,
        timeout: Duration,
    ) -> Result<Option<WarmPodClaim>, WarmPoolError> {
        let inner = self.inner.with_resource_receipts();
        let result = self
            .try_claim_with_receipts(&inner, digest, lease_id, tenant_id, policy, timeout)
            .await;
        if result.is_err() {
            let resources = inner.resource_receipts();
            if let Err(error) = self.inner.release_resources(&resources).await {
                self.pending_cleanup.lock().await.push(resources);
                warn!(%error, "retaining failed warm claim cleanup");
            }
        }
        result
    }

    pub async fn try_claim_with_receipts(
        &self,
        inner: &K8sMicroVmProvider,
        digest: &Digest,
        lease_id: &str,
        tenant_id: &str,
        policy: &NetworkPolicy,
        timeout: Duration,
    ) -> Result<Option<WarmPodClaim>, WarmPoolError> {
        let digest_str = digest.to_string();
        let claimed = inner.claim_warm_pod(&digest_str, lease_id, tenant_id, timeout).await?;
        let Some(claimed) = claimed else {
            return Ok(None);
        };
        let pod_name = claimed.name;
        let secret_name = claimed.restore_secret_name;
        let owner = inner
            .resource_receipts()
            .into_iter()
            .find(|receipt| receipt.kind() == K8sResourceKind::Pod && receipt.name() == pod_name)
            .ok_or(WarmPoolError::ClaimWithoutPod)?;
        if let Some(secret_name) = &secret_name {
            inner.record_owned_secret(&owner, secret_name).await?;
        }
        let network_policy_name = inner
            .apply_network_policy_to_claimed_warm_pod(&pod_name, lease_id, policy)
            .await?;

        self.decrement_population(digest).await;
        info!(
            pod = %pod_name,
            digest = %digest_str,
            lease_id = %lease_id,
            "claimed warm pool pod"
        );
        Ok(Some(WarmPodClaim {
            pod_name,
            secret_name,
            network_policy_name,
            resources: inner.resource_receipts(),
        }))
    }

    pub fn spawn_controller(self: Arc<Self>, cancel: CancellationToken) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(WARM_POOL_RECONCILE_INTERVAL);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        if let Err(e) = self.reconcile().await {
                            warn!(error = %e, "warm pool reconcile failed");
                        }
                    }
                }
            }
        });
    }

    pub async fn reconcile(&self) -> Result<(), WarmPoolError> {
        let pending = std::mem::take(&mut *self.pending_cleanup.lock().await);
        for resources in pending {
            if let Err(error) = self.inner.release_resources(&resources).await {
                self.pending_cleanup.lock().await.push(resources);
                warn!(%error, "warm-pool cleanup remains pending");
            }
        }
        // Count first: the pool's slots live on the pods themselves (labels
        // and annotations), so a restarted worker learns what it already has
        // before it decides to create or delete anything.
        self.sync_population_from_cluster().await?;
        self.evict_stale_idle_pods().await?;
        self.sync_population_from_cluster().await?;
        self.top_up_pools().await?;
        Ok(())
    }

    /// Deletes idle warm pods older than `max_idle_seconds`.
    ///
    /// Both sides of the age comparison come from the apiserver: its `Date`
    /// header for `now`, its `creationTimestamp` for the pod. A worker host's
    /// clock is never consulted, because several replicas run this sweep and a
    /// host skewed fast would not race with its peers -- it would compute every
    /// pod they just created as expired, on every pass. When the apiserver
    /// declines to say what time it is, the sweep is skipped: a warm pod that
    /// lingers costs one idle pod, one deleted too early costs a live sandbox.
    async fn evict_stale_idle_pods(&self) -> Result<(), WarmPoolError> {
        let listing = self.inner.list_warm_pool_pods().await?;
        let Some(now) = listing.clock else {
            warn!("apiserver returned no usable Date header, skipping warm pool eviction");
            return Ok(());
        };
        for pod in listing.pods {
            let labels = pod.metadata.labels.as_ref();
            let Some(labels) = labels else {
                continue;
            };
            if labels.get(LABEL_WARM_POOL).map(String::as_str) != Some(WARM_POOL_IDLE) {
                continue;
            }
            let Some(name) = pod.metadata.name.clone() else {
                continue;
            };
            let Some(age_seconds) = pod_age_seconds(&pod, now) else {
                continue;
            };
            if age_seconds <= self.config.max_idle_seconds as i64 {
                continue;
            }
            debug!(pod = %name, age_seconds, "evicting stale idle warm pod");
            self.delete_idle_pod(&pod).await?;
        }
        Ok(())
    }

    /// Rebuilds the slot counts from the cluster and deletes idle pods that
    /// failed, which no claim or refill would ever use again.
    async fn sync_population_from_cluster(&self) -> Result<(), WarmPoolError> {
        let listing = self.inner.list_warm_pool_pods().await?;
        let mut census = SlotCensus::default();
        for pod in &listing.pods {
            let state = WarmSlotState::of(
                pod.metadata.labels.as_ref(),
                pod.status.as_ref().and_then(|s| s.phase.as_deref()),
                pod.metadata.deletion_timestamp.is_some(),
            );
            let digest = pod
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(ANNOTATION_DIGEST))
                .and_then(|d| Digest::new(d).ok());
            if state == Some(WarmSlotState::Failed)
                && let Err(error) = self.delete_idle_pod(pod).await
            {
                warn!(pod = ?pod.metadata.name, error = %error, "failed to delete a failed warm pod");
            }
            if matches!(state, Some(WarmSlotState::Ready | WarmSlotState::Warming))
                && let Some(digest) = &digest
            {
                self.recover_digest(digest, pod.metadata.annotations.as_ref());
            }
            census.add(state, digest);
        }
        *self.population.lock().await = census.ready;
        *self.warming.lock().await = census.warming;
        Ok(())
    }

    /// Relearns a digest a surviving idle pod was built from, so a restarted
    /// worker keeps that pool filled and pulls from the right repository. A
    /// digest this process already knows is left alone.
    fn recover_digest(&self, digest: &Digest, annotations: Option<&BTreeMap<String, String>>) {
        if self.digest_cache.contains(digest) {
            return;
        }
        self.digest_cache.insert(digest.clone());
        if let Some(reference) = recovered_source(annotations, digest) {
            self.remember_source(&reference);
        }
    }

    async fn top_up_pools(&self) -> Result<(), WarmPoolError> {
        let digests = self.digest_cache.list();
        let ready = self.population.lock().await.clone();
        let warming = self.warming.lock().await.clone();
        for (digest, count) in plan_refill(&self.config, &digests, &ready, &warming) {
            for _ in 0..count {
                match self.create_warm_pod(&digest).await {
                    Ok(()) => {
                        *self.warming.lock().await.entry(digest.clone()).or_insert(0) += 1;
                    }
                    Err(e) => {
                        warn!(digest = %digest, error = %e, "failed to create warm pod");
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    async fn create_warm_pod(&self, digest: &Digest) -> Result<(), WarmPoolError> {
        let reference = self.source_for(digest);
        let tar_bytes = pull_rootfs(self.oci.as_ref(), &reference).await?;
        let source = (reference.registry.as_str(), reference.repository.as_str());
        let secret_name = new_warm_restore_secret_name();
        let inner = self.inner.with_resource_receipts();
        let result = async {
            inner
                .create_workspace_tar_secret(&secret_name, &secret_name, &tar_bytes)
                .await?;
            inner
                .create_warm_pod_from(
                    &digest.to_string(),
                    Some(source),
                    &self.default_image,
                    &secret_name,
                    &self.runtime_class,
                    self.warm_cpu,
                    self.warm_memory_mib,
                )
                .await
        }
        .await;
        match result {
            Ok(pod_name) => {
                debug!(pod = %pod_name, digest = %digest, "created warm pool pod");
                Ok(())
            }
            Err(error) => {
                let resources = inner.resource_receipts();
                if let Err(cleanup) = self.inner.release_resources(&resources).await {
                    self.pending_cleanup.lock().await.push(resources);
                    warn!(%cleanup, "retaining partial warm-pool allocation cleanup");
                }
                Err(error.into())
            }
        }
    }

    async fn delete_idle_pod(&self, pod: &k8s_openapi::api::core::v1::Pod) -> Result<(), WarmPoolError> {
        let inner = self.inner.with_resource_receipts();
        match inner.delete_idle_warm_pod(pod).await {
            Ok(_) => Ok(()),
            Err(error) => {
                let resources = inner.resource_receipts();
                if !resources.is_empty() {
                    self.pending_cleanup.lock().await.push(resources);
                }
                Err(error.into())
            }
        }
    }

    async fn decrement_population(&self, digest: &Digest) {
        let mut pop = self.population.lock().await;
        if let Some(count) = pop.get_mut(digest) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                pop.remove(digest);
            }
        }
    }
}
