use std::collections::{BTreeMap, HashMap, HashSet};
use std::mem;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{info, warn};
use trogon_sandbox_contract::{Digest, OciArtifactRef};
use trogon_sandbox_volumes::VolumeClientConfig;

use crate::constants::{
    ALLOCATION_ANNOTATION, CLAIM_ANNOTATION, LEASE_LABEL, WARM_POOL_IDLE, WARM_POOL_LABEL, WARM_POOL_SLOT_LABEL,
    WARM_POOL_UNASSIGNED_LEASE,
};
use crate::{
    CustodyToken, KubeVirtOps, KubeVirtOpsError, LeaseResources, WarmCandidate, WarmClaim, WarmDiskSource, WarmEntry,
    WarmPoolConfig, WarmPoolManagerError, resource_name,
};

/// Pre-started KubeVirt VMs, keyed by the digest of the snapshot their root disk restores.
///
/// Idle VMs carry [`crate::WARM_POOL_LABEL`]`=`[`crate::WARM_POOL_IDLE`] and a digest annotation. A snapshot
/// lease takes one over by relabelling it to the lease in a single claim fenced on the VM's uid and resource
/// version, so a warm VM goes to at most one lease across every worker. Cleanup that fails is retained and
/// retried by [`WarmPoolManager::reconcile_from_cluster`].
#[derive(Clone)]
pub struct WarmPoolManager {
    config: WarmPoolConfig,
    ops: Arc<KubeVirtOps>,
    source: Arc<dyn WarmDiskSource>,
    entries: Arc<Mutex<HashMap<String, WarmEntry>>>,
    retained: Arc<Mutex<Vec<LeaseResources>>>,
}

impl WarmPoolManager {
    /// A pool that boots warm VMs through `ops` from root disks `source` describes.
    pub fn new(config: WarmPoolConfig, ops: Arc<KubeVirtOps>, source: Arc<dyn WarmDiskSource>) -> Self {
        Self {
            config,
            ops,
            source,
            entries: Arc::default(),
            retained: Arc::default(),
        }
    }

    pub fn config(&self) -> &WarmPoolConfig {
        &self.config
    }

    /// How many warm VMs the pool tracks for each digest.
    pub async fn population_report(&self) -> HashMap<Digest, u32> {
        let entries = self.entries.lock().await;
        let mut counts: HashMap<Digest, u32> = HashMap::new();
        for entry in entries.values() {
            *counts.entry(entry.digest.clone()).or_insert(0) += 1;
        }
        counts
    }

    /// Hands the lease `lease_id` of `tenant` an idle warm VM restored from `digest` that fits `memory_mib`, if
    /// one is free. Whatever a failed claim left owned is released, or retained for the next reconcile.
    pub async fn try_claim(
        &self,
        digest: &Digest,
        memory_mib: u32,
        lease_id: &str,
        tenant: &str,
    ) -> Result<Option<WarmClaim>, KubeVirtOpsError> {
        let ops = self.ops.with_resource_receipts();
        let result = self
            .try_claim_with_receipts(&ops, digest, memory_mib, lease_id, tenant)
            .await;
        if result.is_err() {
            self.retain_failed_cleanup(&ops).await;
        }
        result
    }

    /// [`WarmPoolManager::try_claim`] through `ops`, whose receipt collector keeps what the claim owns. The
    /// caller settles a failure.
    pub async fn try_claim_with_receipts(
        &self,
        ops: &KubeVirtOps,
        digest: &Digest,
        memory_mib: u32,
        lease_id: &str,
        tenant: &str,
    ) -> Result<Option<WarmClaim>, KubeVirtOpsError> {
        if !self.config.fits(memory_mib) {
            return Ok(None);
        }
        let Some(candidate) = ops.find_confirmed_warm_vm(digest, memory_mib).await? else {
            return Ok(None);
        };
        let claim = WarmClaim {
            vm_name: candidate.vm.name.clone(),
            dv_name: candidate.data_volume.name.clone(),
        };
        let labels = ops.lease_labels(lease_id, tenant);
        if !ops.claim_confirmed_warm_vm(candidate, labels).await? {
            return Ok(None);
        }
        self.entries.lock().await.remove(&claim.vm_name);
        info!(vm_name = %claim.vm_name, digest = %digest, lease_id = %lease_id, "claimed warm VMI from pool");
        Ok(Some(claim))
    }

    async fn retain_failed_cleanup(&self, ops: &KubeVirtOps) {
        let resources = ops.resource_receipts();
        if resources.is_empty() {
            return;
        }
        if let Err(error) = self.ops.release_resources(&resources).await {
            self.retained.lock().await.push(resources);
            warn!(error = %error, "retaining warm KubeVirt cleanup receipts");
        }
    }

    /// Starts a warm VM for `reference` in the background once a lease of it ends, unless the pool is disabled or
    /// full. The returned handle finishes when the attempt does; a failed attempt is only logged.
    pub fn spawn_prewarm_after_lease(self: &Arc<Self>, reference: OciArtifactRef) -> JoinHandle<()> {
        let pool = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = pool.maybe_prewarm(&reference).await {
                warn!(digest = %reference.digest, error = %error, "warm pool prewarm skipped");
            }
        })
    }

    async fn maybe_prewarm(&self, reference: &OciArtifactRef) -> Result<(), WarmPoolManagerError> {
        let room = self.config.enabled
            && self.count_for_digest(&reference.digest).await < self.config.max_pool_size_per_digest
            && self.total_warm_count().await < self.config.max_total_warm;
        if !room {
            return Ok(());
        }
        self.create_warm_vmi(reference).await
    }

    async fn count_for_digest(&self, digest: &Digest) -> u32 {
        let entries = self.entries.lock().await;
        let count = entries.values().filter(|entry| entry.digest == *digest).count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    async fn total_warm_count(&self) -> u32 {
        let count = self.entries.lock().await.len();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    async fn create_warm_vmi(&self, reference: &OciArtifactRef) -> Result<(), WarmPoolManagerError> {
        let volume = self.ops.config().volume.as_ref();
        let volume = volume.ok_or(WarmPoolManagerError::VolumeUnconfigured)?;
        let slot_id = format!("warm-{}", CustodyToken::generate());
        let ops = self.ops.with_resource_receipts();
        let candidate = match self.boot_warm_vm(&ops, volume, reference, &slot_id).await {
            Ok(candidate) => candidate,
            Err(error) => {
                self.retain_failed_cleanup(&ops).await;
                return Err(error);
            }
        };
        let vm_name = candidate.vm.name.clone();
        let digest = candidate.digest.clone();
        let entry = WarmEntry {
            digest: digest.clone(),
            candidate,
            idle_since: Instant::now(),
        };
        self.entries.lock().await.insert(vm_name.clone(), entry);
        info!(vm_name = %vm_name, digest = %digest, "warm VMI ready in pool");
        Ok(())
    }

    /// Restores `reference` into a fresh DataVolume and boots an idle warm VM of slot `slot_id` from it.
    async fn boot_warm_vm(
        &self,
        ops: &KubeVirtOps,
        volume: &VolumeClientConfig,
        reference: &OciArtifactRef,
        slot_id: &str,
    ) -> Result<WarmCandidate, WarmPoolManagerError> {
        let vm_name = resource_name("vm", slot_id);
        let dv_name = resource_name("dv", slot_id);
        let digest = &reference.digest;
        let mut labels = ops.warm_pool_idle_labels(digest);
        labels.insert(WARM_POOL_SLOT_LABEL.to_owned(), slot_id.to_owned());
        let allocation = (ALLOCATION_ANNOTATION.to_owned(), slot_id.to_owned());
        let source = self
            .source
            .data_volume(ops, volume, reference, &dv_name, labels.clone());
        let mut data_volume = source.await?;
        data_volume.metadata.annotations = Some(BTreeMap::from([allocation.clone()]));
        let created = ops.create_data_volume_owned(data_volume).await?;
        let disk = ops.receipt(&dv_name, &created)?;
        ops.wait_data_volume_ready(&dv_name).await?;
        let cloud_init = ops.build_cloud_init_user_data(&[]);
        let memory_mib = self.config.warm_memory_mib;
        let mut vm = ops.build_virtual_machine(&vm_name, &dv_name, memory_mib, None, &cloud_init, labels);
        let mut annotations = ops.warm_pool_annotations(digest, &disk, memory_mib);
        annotations.extend([allocation]);
        vm.metadata.annotations = Some(annotations);
        let created = ops.create_virtual_machine_owned(vm).await?;
        ops.validate_virtual_machine(&created)?;
        ops.wait_vmi_ready(&vm_name).await?;
        let owner = ops.receipt(&vm_name, &created)?;
        let revision = created.metadata.resource_version;
        let revision = revision.ok_or(WarmPoolManagerError::VmRevisionMissing)?;
        Ok(WarmCandidate {
            vm: owner,
            data_volume: disk,
            resource_version: revision,
            digest: digest.clone(),
        })
    }

    /// Deletes the warm VMs idle for at least [`WarmPoolConfig::max_idle_seconds`] that are still the idle
    /// revision the pool saw, and returns how many it deleted. A failed eviction is logged, and retained when it
    /// left anything owned.
    pub async fn evict_idle(&self) -> u32 {
        if !self.config.enabled {
            return 0;
        }
        let max_idle = Duration::from_secs(self.config.max_idle_seconds);
        let now = Instant::now();
        let stale: Vec<(String, WarmCandidate)> = self
            .entries
            .lock()
            .await
            .iter()
            .filter(|(_, entry)| now.duration_since(entry.idle_since) >= max_idle)
            .map(|(name, entry)| (name.clone(), entry.candidate.clone()))
            .collect();
        let mut evicted = 0;
        for (vm_name, candidate) in stale {
            let ops = self.ops.with_resource_receipts();
            match ops.evict_confirmed_idle_vm(&candidate).await {
                Ok(true) => {
                    self.entries.lock().await.remove(&vm_name);
                    evicted += 1;
                }
                Ok(false) => {}
                Err(error) => {
                    let pending = ops.resource_receipts();
                    if !pending.is_empty() {
                        self.retained.lock().await.push(pending);
                        self.entries.lock().await.remove(&vm_name);
                    }
                    warn!(vm_name = %vm_name, error = %error, "warm pool eviction remains pending");
                }
            }
        }
        evicted
    }

    /// Retries retained cleanup, then brings the tracked warm VMs in line with the cluster: claimed VMs are
    /// dropped, VMs that are gone or replaced are retained for cleanup, and confirmed idle VMs this worker did not
    /// track yet are adopted.
    pub async fn reconcile_from_cluster(&self) -> Result<(), KubeVirtOpsError> {
        let pending = mem::take(&mut *self.retained.lock().await);
        for resources in pending {
            if let Err(error) = self.ops.release_resources(&resources).await {
                self.retained.lock().await.push(resources);
                warn!(error = %error, "warm KubeVirt cleanup remains pending");
            }
        }
        let listed = self.ops.confirmed_idle_warm_vms().await?;
        let names: HashSet<&str> = listed.iter().map(|candidate| candidate.vm.name.as_str()).collect();
        let missing: Vec<(String, WarmCandidate)> = self
            .entries
            .lock()
            .await
            .iter()
            .filter(|(name, _)| !names.contains(name.as_str()))
            .map(|(name, entry)| (name.clone(), entry.candidate.clone()))
            .collect();
        for (name, candidate) in missing {
            self.reconcile_missing(&name, candidate).await?;
        }
        let mut replaced = Vec::new();
        let mut entries = self.entries.lock().await;
        for candidate in listed {
            let name = candidate.vm.name.clone();
            match entries.get_mut(&name) {
                Some(entry)
                    if entry.candidate.vm == candidate.vm && entry.candidate.data_volume == candidate.data_volume =>
                {
                    entry.candidate = candidate;
                }
                _ => {
                    if let Some(old) = entries.remove(&name) {
                        replaced.push(LeaseResources::claiming(&old.candidate));
                    }
                    let entry = WarmEntry {
                        digest: candidate.digest.clone(),
                        candidate,
                        idle_since: Instant::now(),
                    };
                    entries.insert(name, entry);
                }
            }
        }
        drop(entries);
        self.retained.lock().await.extend(replaced);
        Ok(())
    }

    /// Settles a tracked warm VM the cluster no longer lists as a confirmed idle warm VM.
    async fn reconcile_missing(&self, name: &str, candidate: WarmCandidate) -> Result<(), KubeVirtOpsError> {
        let observed = self.ops.virtual_machine(name).await?;
        let current = match observed {
            Some(observed) if self.ops.receipt(name, &observed)? == candidate.vm => observed,
            _ => {
                self.retained.lock().await.push(LeaseResources::claiming(&candidate));
                self.entries.lock().await.remove(name);
                return Ok(());
            }
        };
        let metadata = &current.metadata;
        let label = |key: &str| metadata.labels.as_ref().and_then(|labels| labels.get(key));
        let idle = label(WARM_POOL_LABEL).is_some_and(|value| value == WARM_POOL_IDLE);
        let claim = metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(CLAIM_ANNOTATION));
        let leased = label(LEASE_LABEL).is_some_and(|lease| lease != WARM_POOL_UNASSIGNED_LEASE && !lease.is_empty());
        let claimed = claim.is_some_and(|claim| !claim.is_empty()) && leased;
        let revision = metadata.resource_version.clone();
        let mut entries = self.entries.lock().await;
        if !idle && claimed {
            entries.remove(name);
        } else if let (true, Some(revision), Some(entry)) = (idle, revision, entries.get_mut(name)) {
            entry.candidate.resource_version = revision;
        }
        Ok(())
    }
}
