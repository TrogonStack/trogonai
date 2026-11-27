//! The lease lifecycle: one microVM per lease, from a rootfs copy to a bound guest agent.

mod constants;
mod guest_lifecycle;
mod retained_snapshots;
use constants::NO_TTL_RESERVATION_SPAN;
use guest_lifecycle::*;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};
use trogon_sandbox_actor_agent::{BindLeaseRequest, GuestEntropySeed};
use trogon_sandbox_contract::{
    ArtifactType, Digest, LeaseError, LeaseId, LeaseRequest, NetworkPolicy, NetworkPolicyUpdateResponse, ProviderKind,
    RunCommandRequest, RunCommandResponse, SnapshotExportError,
};
use trogon_sandbox_contract::{ProviderLease, SandboxPhase};
use trogon_sandbox_protocol::ProviderError as ProviderFailure;
use trogon_sandbox_protocol::{LifecycleEmitter, Provider, run_phase};

use crate::agent::{DiscardChunks, GuestAgent};
use crate::api::FirecrackerApi;
use crate::capability_check::{PolicyRequirements, ProfileSatisfies};
use crate::config::{FirecrackerConfig, MemoryMib};
use crate::error::ProviderError;
use crate::process::{VmExit, VmLauncher, VmProcess, reap_orphan};
use crate::spec::{self, AcceptedSpec};
use crate::vm_dir::{VmDir, stale_dirs};
use crate::vsock::VsockEndpoint;

/// Builds the per-lease lifecycle emitter; the subject carries the lease id.
#[derive(Clone)]
struct EmitterFactory {
    js: async_nats::jetstream::Context,
}

impl EmitterFactory {
    fn build(&self, lease_id: &LeaseId) -> Arc<dyn LifecycleEmitter> {
        Arc::new(crate::nats_lifecycle::NatsLifecycleEmitter::new(
            self.js.clone(),
            trogon_sandbox_protocol::lifecycle_subject(lease_id),
        ))
    }
}

/// Why a lease this worker granted can no longer be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseLoss {
    /// The VMM exited while the lease was still held.
    VmExited { status: String },
    /// A previous run of this worker started the VM, and VMs are not handed over.
    WorkerRestarted,
    /// The worker shut down, taking its VMs with it.
    WorkerStopped,
}

impl fmt::Display for LeaseLoss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VmExited { status } => write!(f, "the microVM stopped unexpectedly ({status})"),
            Self::WorkerRestarted => write!(f, "the worker restarted and reclaimed the microVM"),
            Self::WorkerStopped => write!(f, "the worker shut down and stopped the microVM"),
        }
    }
}

/// Told about every lease that ends without being cancelled, so whoever granted it can
/// release it instead of waiting for its TTL.
#[async_trait]
pub trait LeaseLossSink: Send + Sync {
    async fn lost(&self, lease_id: &LeaseId, loss: &LeaseLoss);
}

/// Why a running lease could not be checkpointed.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("this worker does not hold the lease")]
    NotHeld,
    #[error("{0}")]
    Unsupported(String),
    #[error("checkpointing the microVM failed: {0}")]
    Failed(String),
}

impl CheckpointError {
    pub fn to_wire(&self) -> SnapshotExportError {
        let message = self.to_string();
        match self {
            Self::Unsupported(_) => SnapshotExportError::Unsupported { message },
            Self::NotHeld | Self::Failed(_) => SnapshotExportError::Permanent { message },
        }
    }
}

fn failed(error: impl fmt::Display) -> CheckpointError {
    CheckpointError::Failed(error.to_string())
}

/// What a verb for a lease can reach on this worker.
pub enum LeaseAccess {
    Live(GuestAgent),
    Lost(LeaseLoss),
    NotHeld,
}

struct ActiveVm {
    resource: Option<crate::attempt_resources::ResourceReceipt>,
    generation: u64,
    dir: VmDir,
    memory: MemoryMib,
    process: VmProcess,
    agent: GuestAgent,
    image_defaults: Option<trogon_sandbox_oci::ResolvedContainerImage>,
    network: Option<crate::network::LeaseNetwork>,
    sandbox_id: String,
    snapshot_policy: Option<trogon_sandbox_contract::SnapshotPolicy>,
    _disk_space: Option<Arc<crate::space::DiskReservation>>,
    /// Held for the VM's lifetime so the concurrency cap counts live VMs, not requests.
    _permit: OwnedSemaphorePermit,
}

#[derive(Default)]
struct StartupRecovery {
    lock: Option<crate::state::StateDirectoryLock>,
    ready: bool,
}

struct BootedVm {
    agent: GuestAgent,
    image_defaults: Option<trogon_sandbox_oci::ResolvedContainerImage>,
}

pub struct FirecrackerProvider {
    resources: Arc<crate::attempt_resources::AttemptResources>,
    config: Arc<FirecrackerConfig>,
    launcher: Arc<dyn VmLauncher>,
    lifecycle: Option<EmitterFactory>,
    slots: Arc<Semaphore>,
    active: Arc<Mutex<HashMap<String, ActiveVm>>>,
    losses: Arc<Mutex<HashMap<String, LeaseLoss>>>,
    generations: AtomicU64,
    loss_sink: Option<Arc<dyn LeaseLossSink>>,
    snapshots: Option<crate::snapshot::SnapshotStore>,
    checkpoints: Mutex<()>,
    oci: Arc<trogon_sandbox_oci::OciClient>,
    warm_pool: Mutex<crate::warm_pool::WarmPool>,
    startup: Mutex<StartupRecovery>,
    images: Option<crate::rootfs::ImageCache>,
    dispatch_ledger: tokio::sync::OnceCell<Arc<trogon_sandbox_protocol::AttemptLedger>>,
    retained: tokio::sync::OnceCell<Arc<crate::retained::SnapshotDepot>>,
    lease_gates: Arc<crate::retained::LeaseGates>,
    committed_fence: Option<trogon_sandbox_credential_broker::CommittedFence>,
}

impl FirecrackerProvider {
    pub fn new(config: FirecrackerConfig, launcher: Arc<dyn VmLauncher>) -> Self {
        let resources = Arc::new(crate::attempt_resources::AttemptResources::new(
            config.state_dir.as_path().to_path_buf(),
        ));
        let slots = Arc::new(Semaphore::new(config.max_concurrent_leases as usize));
        let warm_pool = Mutex::new(crate::warm_pool::WarmPool::new(config.warm_pool.clone()));
        let oci = Arc::new(trogon_sandbox_oci::OciClient::new(Default::default()));
        let images = config
            .oci_rootfs
            .clone()
            .map(|config| crate::rootfs::ImageCache::new(config, Arc::clone(&oci)));
        let snapshots = config.snapshots.clone().map(crate::snapshot::SnapshotStore::new);
        Self {
            resources,
            snapshots,
            checkpoints: Mutex::new(()),
            losses: Arc::new(Mutex::new(HashMap::new())),
            generations: AtomicU64::new(0),
            loss_sink: None,
            config: Arc::new(config),
            launcher,
            lifecycle: None,
            slots,
            active: Arc::new(Mutex::new(HashMap::new())),
            oci,
            warm_pool,
            startup: Mutex::new(StartupRecovery::default()),
            images,
            dispatch_ledger: tokio::sync::OnceCell::new(),
            retained: tokio::sync::OnceCell::new(),
            lease_gates: Default::default(),
            committed_fence: None,
        }
    }

    pub fn with_lifecycle_stream(mut self, js: async_nats::jetstream::Context) -> Self {
        self.lifecycle = Some(EmitterFactory { js });
        self
    }

    /// Stamps the committed credential policy revision on each lease's credential requests,
    /// and with the fence required refuses a lease that needs credentials but has none.
    pub fn with_committed_fence(mut self, fence: trogon_sandbox_credential_broker::CommittedFence) -> Self {
        self.committed_fence = Some(fence);
        self
    }

    pub fn with_loss_sink(mut self, sink: Arc<dyn LeaseLossSink>) -> Self {
        self.loss_sink = Some(sink);
        self
    }
    pub async fn cached_digests(&self) -> Vec<Digest> {
        match &self.snapshots {
            Some(store) => store.digests().await,
            None => Vec::new(),
        }
    }
    pub async fn answers_for(&self, lease: &LeaseId) -> bool {
        self.active.lock().await.contains_key(lease.as_str()) || self.losses.lock().await.contains_key(lease.as_str())
    }
    pub async fn access(&self, lease: &LeaseId) -> LeaseAccess {
        if let Some(loss) = self.losses.lock().await.get(lease.as_str()).cloned() {
            return LeaseAccess::Lost(loss);
        }
        match self.active.lock().await.get(lease.as_str()) {
            Some(vm) => LeaseAccess::Live(vm.agent.clone()),
            None => LeaseAccess::NotHeld,
        }
    }
    async fn record_loss(&self, lease: &LeaseId, loss: LeaseLoss) {
        self.losses.lock().await.insert(lease.as_str().to_owned(), loss.clone());
        if let Some(sink) = &self.loss_sink {
            sink.lost(lease, &loss).await;
        }
    }
    async fn recover_network(&self, dir: &VmDir) -> anyhow::Result<()> {
        crate::network::recover(dir).await?;
        if let Some(config) = &self.config.legacy_network {
            crate::network::recover_helper(config, dir).await?;
        }
        Ok(())
    }

    pub async fn update_network_policy(
        &self,
        lease: &LeaseId,
        policy: &NetworkPolicy,
    ) -> Option<NetworkPolicyUpdateResponse> {
        let mut active = self.active.lock().await;
        let Some(vm) = active.get_mut(lease.as_str()) else {
            return self.losses.lock().await.get(lease.as_str()).map(|loss| {
                NetworkPolicyUpdateResponse::Denied(LeaseError::Backend {
                    message: loss.to_string(),
                })
            });
        };
        if !self
            .profile()
            .satisfies(&trogon_sandbox_contract::CapabilityRequirements::default().with_network_policy(policy))
        {
            return Some(NetworkPolicyUpdateResponse::Denied(LeaseError::InvalidNetworkPolicy {
                reason: "network policy requires unavailable Firecracker proxy capabilities".into(),
            }));
        }
        Some(match vm.network.as_mut() {
            None if matches!(policy, NetworkPolicy::DenyAll) => NetworkPolicyUpdateResponse::Applied,
            None => NetworkPolicyUpdateResponse::Denied(LeaseError::InvalidNetworkPolicy {
                reason: "this lease was booted without a network device, which cannot be added while it runs".into(),
            }),
            Some(network) => match network.update_policy(policy).await {
                Ok(()) => NetworkPolicyUpdateResponse::Applied,
                Err(error) => NetworkPolicyUpdateResponse::Denied(LeaseError::Backend {
                    message: error.to_string(),
                }),
            },
        })
    }

    pub async fn checkpoint_inner(
        &self,
        lease: &LeaseId,
    ) -> Result<trogon_sandbox_contract::SnapshotReceipt, CheckpointError> {
        let store = self
            .snapshots
            .as_ref()
            .ok_or_else(|| CheckpointError::Unsupported("this worker has no --snapshot-dir local store".into()))?;
        let _checkpoint = self.checkpoints.lock().await;
        let _gate = self.lease_gates.lock(lease).await;
        {
            let active = self.active.lock().await;
            match active.get(lease.as_str()) {
                Some(vm) if vm.network.is_some() => {
                    return Err(CheckpointError::Unsupported(
                        "a lease with a network device cannot be checkpointed through the local snapshot store".into(),
                    ));
                }
                Some(_) => (),
                None => return Err(CheckpointError::NotHeld),
            }
        }
        let captured = self.capture_active(lease).await.map_err(failed)?;
        let stored = store.commit_captured(captured).await.map_err(failed)?;
        Ok(trogon_sandbox_contract::SnapshotReceipt {
            digest: stored.digest,
            bytes: stored.bytes,
            artifact_type: ArtifactType::FirecrackerSnapshotV1,
        })
    }

    pub fn profile(&self) -> trogon_sandbox_contract::ProviderCapabilityProfile {
        let mut profile = crate::profile::PROFILE.clone();
        profile.placement.env_digest = self.config.environment.own.clone();
        crate::profile::with_network(&mut profile, &self.config);
        profile
    }

    pub fn config(&self) -> &FirecrackerConfig {
        &self.config
    }

    pub async fn dispatch_ledger(&self) -> anyhow::Result<&Arc<trogon_sandbox_protocol::AttemptLedger>> {
        self.recover_stale().await?;
        self.dispatch_ledger
            .get_or_try_init(|| async {
                Ok(Arc::new(trogon_sandbox_protocol::AttemptLedger::open(
                    self.config.state_dir.as_path().join("dispatch.json"),
                    ProviderKind::Firecracker,
                    None,
                )?))
            })
            .await
    }

    pub async fn lease_dispatch(
        &self,
        payload: &trogon_sandbox_contract::ProviderDispatchPayload,
    ) -> anyhow::Result<ProviderLease> {
        let target = payload
            .target
            .clone()
            .ok_or_else(|| anyhow::anyhow!("targeted dispatch identity is required"))?;
        spec::accept(&self.config, &payload.request)?;
        anyhow::ensure!(
            payload.resolved_drives.is_empty(),
            "native drive records are unavailable on Firecracker"
        );
        let ledger = self.dispatch_ledger().await?;
        let identity = trogon_sandbox_contract::LeaseAttemptIdentity {
            lease_id: payload.lease_id.clone(),
            provider: ProviderKind::Firecracker,
            target,
        };
        ledger
            .dispatch(&identity, || async {
                let receipt = lease_of(payload.lease_id.as_str(), &payload.lease_id)
                    .map_err(anyhow::Error::into_boxed_dyn_error)?;
                ledger
                    .record_backend_receipt(&identity, receipt)
                    .await
                    .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
                self.lease_inner_for_attempt(&payload.request, &payload.lease_id, Some(&identity))
                    .await
                    .map_err(Box::<dyn std::error::Error + Send + Sync>::from)
            })
            .await
            .map_err(|error| match error {
                trogon_sandbox_protocol::AttemptLedgerError::Allocation(source) => {
                    match source.downcast::<ProviderError>() {
                        Ok(refusal) => anyhow::Error::from(*refusal),
                        Err(source) => anyhow::Error::from_boxed(source),
                    }
                }
                other => anyhow::Error::from(other),
            })
    }

    pub async fn cleanup_dispatch(
        &self,
        request: &trogon_sandbox_contract::ProviderCleanupRequest,
    ) -> trogon_sandbox_contract::ProviderCleanupResponse {
        let ledger = match self.dispatch_ledger().await {
            Ok(ledger) => ledger,
            Err(error) => {
                return trogon_sandbox_contract::ProviderCleanupResponse::Failed {
                    identity: request.identity.clone(),
                    message: error.to_string(),
                };
            }
        };
        let response = ledger.cleanup(request, |state| self.cleanup_resource(state)).await;
        if matches!(
            response,
            trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { .. }
        ) {
            let keep = if request.intent == trogon_sandbox_contract::CancelIntent::Release {
                self.has_retained_record(&request.identity.lease_id)
                    .await
                    .unwrap_or(true)
            } else {
                false
            };
            if !keep && let Err(error) = self.resources.forget(&request.identity).await {
                warn!(%error, "completed VM resource proof remains pending removal");
            }
        }
        response
    }

    async fn cleanup_resource(&self, state: trogon_sandbox_protocol::CleanupState) -> anyhow::Result<()> {
        let lease = &state.identity.lease_id;
        let _gate = self.lease_gates.lock(lease).await;
        if state.backend_cleaned {
            let resource = self.resources.read_optional(&state.identity).await?;
            if state.intent == trogon_sandbox_contract::CancelIntent::Purge
                && resource.as_ref().is_none_or(|resource| resource.effects_started)
            {
                self.purge_retained_locked(lease, resource.as_ref()).await?;
            }
            return Ok(());
        }
        let resource = self.resources.read(&state.identity).await?;
        if let Some(receipt) = &state.receipt {
            anyhow::ensure!(
                receipt.sandbox_id.as_str() == lease.as_str(),
                "attempt backend identity mismatch"
            );
        }
        if !resource.effects_started {
            anyhow::ensure!(
                !self
                    .active
                    .lock()
                    .await
                    .get(lease.as_str())
                    .is_some_and(|vm| vm.resource.as_ref().is_some_and(|held| held.same_resource(&resource))),
                "no-effect receipt contradicts a live allocation"
            );
            self.resources.complete(&resource).await?;
            return Ok(());
        }
        if resource.settled {
            let mut active = self.active.lock().await;
            if active
                .get(lease.as_str())
                .is_some_and(|vm| vm.resource.as_ref().is_some_and(|held| held.same_resource(&resource)))
            {
                anyhow::ensure!(
                    active[lease.as_str()].process.exit().status().is_some(),
                    "completed VM still has an unresolved process handle"
                );
                active.remove(lease.as_str());
            }
            drop(active);
        } else {
            let held = self.active.lock().await.get(lease.as_str()).map(|vm| {
                (
                    vm.resource.clone(),
                    vm.dir.clone(),
                    vm.process.exit().status().is_some(),
                )
            });
            if let Some((current, dir, exited)) = held {
                anyhow::ensure!(
                    current.as_ref().is_some_and(|current| current.same_resource(&resource)),
                    "lease is held by another VM resource incarnation"
                );
                if dir.as_path().exists() {
                    self.resources.confirm_directory(&dir, &resource).await?;
                } else {
                    anyhow::ensure!(exited, "VM directory absence does not prove process completion");
                }
                match state.intent {
                    trogon_sandbox_contract::CancelIntent::Release => self.cancel_locked(lease).await?,
                    trogon_sandbox_contract::CancelIntent::Purge => {
                        self.purge_retained_locked(lease, Some(&resource)).await?;
                        self.purge_locked(lease).await?;
                    }
                }
            } else {
                let dir = self.launcher.vm_dir(&self.config.state_dir, lease)?;
                self.resources.confirm_directory(&dir, &resource).await?;
                reap_orphan(&dir).await?;
                self.recover_network(&dir).await?;
                self.launcher.cleanup(&dir).await?;
                dir.remove().await?;
                self.resources.complete(&resource).await?;
            }
        }
        if state.intent == trogon_sandbox_contract::CancelIntent::Purge {
            self.purge_retained_locked(lease, Some(&resource)).await?;
        }
        anyhow::ensure!(
            self.resources.read(&state.identity).await?.settled,
            "VM resource cleanup is not positively settled"
        );
        Ok(())
    }

    async fn has_retained_record(&self, lease: &LeaseId) -> anyhow::Result<bool> {
        if self.retained.get().is_none()
            && !self
                .config
                .state_dir
                .as_path()
                .join(crate::retained::DIRECTORY)
                .exists()
        {
            return Ok(false);
        }
        Ok(self.depot().await?.get(lease)?.is_some())
    }

    async fn purge_retained_locked(
        &self,
        lease: &LeaseId,
        expected: Option<&crate::attempt_resources::ResourceReceipt>,
    ) -> anyhow::Result<()> {
        if self.retained.get().is_some()
            || self
                .config
                .state_dir
                .as_path()
                .join(crate::retained::DIRECTORY)
                .exists()
        {
            let depot = self.depot().await?;
            if let Some(record) = depot.get(lease)? {
                anyhow::ensure!(
                    record
                        .resource
                        .as_ref()
                        .zip(expected)
                        .is_some_and(|(actual, expected)| actual.same_resource(expected)),
                    "retained snapshot allocation ownership is unconfirmed"
                );
                depot.mark_purged(lease).await?;
                depot.delete_purged(lease)?;
            }
        }
        Ok(())
    }

    pub async fn current_lease_count(&self) -> u32 {
        self.reap_exited().await;
        let available = u32::try_from(self.slots.available_permits()).unwrap_or(u32::MAX);
        self.config.max_concurrent_leases.saturating_sub(available)
    }

    async fn reap_exited(&self) {
        let exited = {
            let mut active = self.active.lock().await;
            active
                .iter_mut()
                .filter_map(|(id, vm)| {
                    vm.process
                        .has_exited()
                        .then(|| (LeaseId::new(id.clone()), vm.generation, vm.process.exit()))
                })
                .collect::<Vec<_>>()
        };
        for (lease, generation, exit) in exited {
            cleanup_lost(
                Arc::clone(&self.active),
                Arc::clone(&self.losses),
                Arc::clone(&self.launcher),
                Arc::clone(&self.lease_gates),
                self.loss_sink.clone(),
                self.lifecycle.clone(),
                self.resources.clone(),
                lease,
                generation,
                exit.wait().await,
            )
            .await;
        }
    }

    pub async fn holds_lease(&self, lease_id: &LeaseId) -> bool {
        self.active.lock().await.contains_key(lease_id.as_str())
    }

    pub async fn agent_for(&self, lease_id: &LeaseId) -> Option<GuestAgent> {
        match self.access(lease_id).await {
            LeaseAccess::Live(agent) => Some(agent),
            _ => None,
        }
    }

    /// Removes what a previous run left under the state dir. VMs do not survive a worker
    /// restart (nothing here adopts them), so anything found is garbage.
    pub async fn clean_stale_count(&self) -> usize {
        match self.recover_stale().await {
            Ok(cleaned) => cleaned,
            Err(error) => {
                warn!(%error, "worker startup recovery failed");
                0
            }
        }
    }

    pub async fn clean_stale(&self) -> Vec<LeaseId> {
        let before = stale_dirs(&self.config.state_dir).await.unwrap_or_default();
        if self.recover_stale().await.is_err() {
            return Vec::new();
        }
        before.into_iter().filter_map(|dir| dir.lease_id()).collect()
    }

    pub async fn recover_stale(&self) -> Result<usize, ProviderError> {
        let mut startup = self.startup.lock().await;
        if startup.ready {
            return Ok(0);
        }
        if startup.lock.is_none() {
            startup.lock = Some(crate::state::StateDirectoryLock::acquire(&self.config.state_dir).await?);
        }
        let dirs = stale_dirs(&self.config.state_dir)
            .await
            .map_err(|error| ProviderError::Launch(format!("scanning stale leases: {error}")))?;
        let mut cleaned = 0;
        let mut reclaimed = Vec::new();
        for dir in dirs {
            let failed = |error: String| {
                ProviderError::Launch(format!("recovering stale lease {}: {error}", dir.as_path().display()))
            };
            let resource = self
                .resources
                .directory_receipt(&dir)
                .await
                .map_err(|error| failed(error.to_string()))?;
            if let Some(resource) = &resource {
                let original = self
                    .resources
                    .read(&resource.identity)
                    .await
                    .map_err(|error| failed(error.to_string()))?;
                if !original.same_resource(resource) {
                    return Err(failed("VM resource recovery identity differs".into()));
                }
            }
            reap_orphan(&dir).await.map_err(|error| failed(error.to_string()))?;
            self.recover_network(&dir)
                .await
                .map_err(|error| failed(error.to_string()))?;
            self.launcher
                .cleanup(&dir)
                .await
                .map_err(|error| failed(error.to_string()))?;
            dir.remove().await.map_err(|error| failed(error.to_string()))?;
            if let Some(resource) = &resource {
                self.resources
                    .complete(resource)
                    .await
                    .map_err(|error| failed(error.to_string()))?;
            }
            if let Some(lease) = dir.lease_id() {
                reclaimed.push(lease);
            }
            cleaned += 1;
        }
        startup.ready = true;
        drop(startup);
        for lease in reclaimed {
            self.record_loss(&lease, LeaseLoss::WorkerRestarted).await;
        }
        Ok(cleaned)
    }

    /// Stops every VM this worker holds. Called on shutdown, since a restarted worker does
    /// not adopt VMs and would only delete their directories.
    pub async fn shutdown(&self) {
        let held: Vec<String> = self.active.lock().await.keys().cloned().collect();
        for id in held {
            if let Err(e) = self.cancel_inner(&LeaseId::new(id.clone())).await {
                warn!(lease_id = %id, error = %e, "releasing a lease on shutdown failed");
            }
            self.record_loss(&LeaseId::new(id), LeaseLoss::WorkerStopped).await;
        }
    }

    pub async fn lease_inner(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        self.lease_inner_for_attempt(req, lease_id, None).await
    }

    async fn lease_inner_for_attempt(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
        identity: Option<&trogon_sandbox_contract::LeaseAttemptIdentity>,
    ) -> Result<ProviderLease, ProviderError> {
        let _gate = self.lease_gates.lock(lease_id).await;
        let resource = match identity {
            Some(identity) => Some(self.resources.begin(identity).await.map_err(downcast)?),
            None => None,
        };
        self.reap_exited().await;
        if let Some(existing) = self.active.lock().await.get(lease_id.as_str()) {
            if identity.is_some_and(|identity| {
                existing
                    .resource
                    .as_ref()
                    .is_none_or(|resource| resource.identity != *identity)
            }) {
                return Err(ProviderError::Identity("lease belongs to another VM allocation".into()));
            }
            return lease_of(existing.sandbox_id.as_str(), lease_id)
                .map_err(|error| ProviderError::Launch(error.to_string()));
        }

        let accepted = spec::accept(&self.config, req)?;
        let dir = self.launcher.vm_dir(&self.config.state_dir, lease_id)?;
        self.recover_stale().await?;
        if self
            .config
            .state_dir
            .as_path()
            .join(crate::retained::DIRECTORY)
            .exists()
            && self
                .depot()
                .await
                .map_err(downcast)?
                .get(lease_id)
                .map_err(downcast)?
                .is_some()
        {
            return Err(ProviderError::InvalidSpec(
                "lease identity already has a released snapshot record".into(),
            ));
        }
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| ProviderError::AtCapacity {
                max: self.config.max_concurrent_leases,
            })?;
        let resource = match resource {
            Some(resource) => {
                let resource = self.resources.start_effects(&resource).await.map_err(downcast)?;
                dir.create().await?;
                self.resources.attach(&dir, &resource).await.map_err(downcast)?;
                Some(resource)
            }
            None => None,
        };

        let emitter = self.lifecycle.as_ref().map(|f| f.build(lease_id));
        let emitter = emitter.as_deref();

        let mut process: Option<VmProcess> = None;
        let mut network: Option<crate::network::LeaseNetwork> = None;
        let mut disk_space = None;
        let booted = self
            .boot(
                emitter,
                lease_id,
                &accepted,
                &dir,
                &mut process,
                &mut network,
                &mut disk_space,
                (!req.spec.no_ttl().is_unbounded()).then(|| Duration::from_secs(u64::from(req.ttl_seconds))),
            )
            .await;

        let booted = match booted {
            Ok(booted) => booted,
            Err(error) => {
                if let Some(mut process) = process {
                    process.stop().await;
                }
                if let Some(network) = network.as_mut()
                    && let Err(cleanup_error) = network.shutdown(&dir).await
                {
                    warn!(%cleanup_error, "retaining failed network resources for recovery");
                    return Err(error);
                }
                if let Err(cleanup_error) = crate::network::recover(&dir).await {
                    warn!(%cleanup_error, "retaining partial network resources for recovery");
                    return Err(error);
                }
                if let Err(cleanup_error) = self.launcher.cleanup(&dir).await {
                    warn!(lease_id = %lease_id.as_str(), %cleanup_error, "retaining resources after a failed launch");
                    return Err(error);
                }
                if let Err(e) = dir.remove().await {
                    warn!(lease_id = %lease_id.as_str(), error = %e, "cleanup after a failed lease also failed");
                    return Err(error);
                }
                if let Some(depot) = self.retained.get().filter(|depot| {
                    depot
                        .get(lease_id)
                        .ok()
                        .flatten()
                        .is_some_and(|entry| matches!(entry.state, crate::retained::RetainedState::Armed { .. }))
                }) && let Err(cleanup_error) = async {
                    depot.mark_purged(lease_id).await?;
                    depot.delete_purged(lease_id)
                }
                .await
                {
                    warn!(%cleanup_error, "failed lease snapshot reservation cleanup failed");
                }
                if let Some(resource) = &resource
                    && let Err(cleanup_error) = self.resources.complete(resource).await
                {
                    warn!(%cleanup_error, "failed lease completion remains unconfirmed");
                }
                return Err(error);
            }
        };

        let sandbox_id = lease_id.as_str().to_owned();
        info!(lease_id = %lease_id.as_str(), memory_mib = accepted.memory.get(), "microVM ready");
        let Some(process) = process else {
            return Err(ProviderError::Launch("a booted lease has no process".to_string()));
        };
        let exit = process.exit();
        let generation = self.generations.fetch_add(1, Ordering::Relaxed);
        self.losses.lock().await.remove(lease_id.as_str());
        self.active.lock().await.insert(
            lease_id.as_str().to_owned(),
            ActiveVm {
                resource,
                generation,
                dir,
                memory: accepted.memory,
                process,
                agent: booted.agent,
                image_defaults: booted.image_defaults,
                network,
                sandbox_id: sandbox_id.clone(),
                snapshot_policy: accepted.snapshot_out.clone(),
                _disk_space: disk_space,
                _permit: permit,
            },
        );
        self.watch(lease_id.clone(), generation, exit);
        lease_of(&sandbox_id, lease_id).map_err(|error| ProviderError::Launch(error.to_string()))
    }

    fn watch(&self, lease: LeaseId, generation: u64, exit: VmExit) {
        let (active, losses, launcher, gates, sink, lifecycle, resources) = (
            self.active.clone(),
            self.losses.clone(),
            self.launcher.clone(),
            self.lease_gates.clone(),
            self.loss_sink.clone(),
            self.lifecycle.clone(),
            self.resources.clone(),
        );
        tokio::spawn(async move {
            cleanup_lost(
                active,
                losses,
                launcher,
                gates,
                sink,
                lifecycle,
                resources,
                lease,
                generation,
                exit.wait().await,
            )
            .await;
        });
    }

    #[cfg(test)]
    async fn track(
        &self,
        lease: &LeaseId,
        dir: VmDir,
        process: VmProcess,
        network: Option<crate::network::LeaseNetwork>,
        agent: GuestAgent,
        permit: OwnedSemaphorePermit,
    ) -> anyhow::Result<()> {
        let resource = self.resources.directory_receipt(&dir).await?;
        let generation = self.generations.fetch_add(1, Ordering::Relaxed);
        let exit = process.exit();
        self.losses.lock().await.remove(lease.as_str());
        self.active.lock().await.insert(
            lease.as_str().to_owned(),
            ActiveVm {
                resource,
                generation,
                dir,
                process,
                network,
                agent,
                memory: self.config.default_memory,
                image_defaults: None,
                snapshot_policy: None,
                _disk_space: None,
                sandbox_id: lease.as_str().to_owned(),
                _permit: permit,
            },
        );
        self.watch(lease.clone(), generation, exit);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn boot(
        &self,
        emitter: Option<&dyn LifecycleEmitter>,
        lease_id: &LeaseId,
        accepted: &AcceptedSpec,
        dir: &VmDir,
        process: &mut Option<VmProcess>,
        network: &mut Option<crate::network::LeaseNetwork>,
        disk_space: &mut Option<Arc<crate::space::DiskReservation>>,
        lifetime: Option<Duration>,
    ) -> Result<BootedVm, ProviderError> {
        let mut accepted = accepted.clone();
        let local_snapshot = match (&self.snapshots, &accepted.restore) {
            (Some(store), Some(reference)) => store.hold(&reference.digest).await,
            _ => None,
        };
        let base = if let Some(image) = &accepted.registry_image {
            let images = self
                .images
                .as_ref()
                .ok_or_else(|| ProviderError::InvalidSpec("OCI image conversion is not configured".into()))?;
            Some(
                run_phase(emitter, SandboxPhase::PullingImage, None, || async {
                    images.resolve(image).await
                })
                .await
                .map_err(downcast)?,
            )
        } else {
            None
        };
        let mut image_defaults = base.as_ref().map(|base| base.image.clone());
        let base_identity = base
            .as_ref()
            .map(|base| base.identity.to_string())
            .unwrap_or_else(|| format!("preinstalled:{}:{}", self.config.base_rootfs, self.config.default_image));
        let warm_key = crate::warm_pool::WarmKey::new(
            &base_identity,
            &self.config,
            accepted.memory,
            accepted.network_policy.as_ref(),
        );
        let warm = if accepted.restore.is_none() {
            self.warm_pool.lock().await.get(&warm_key)
        } else {
            None
        };
        let warm_enabled = self.warm_pool.lock().await.enabled();
        run_phase(emitter, SandboxPhase::Allocate, None, || async {
            dir.create().await?;
            if let Some(held) = &local_snapshot {
                let files = held.files();
                let metadata = files
                    .restore_metadata(crate::snapshot::RestoreMachine {
                        memory: accepted.memory,
                        vcpus: self.config.default_vcpus,
                        rootfs_max_bytes: self.config.snapshot_rootfs_max_bytes,
                    })
                    .await?;
                crate::environment::check_restore(
                    &self.config.environment,
                    metadata.as_ref().and_then(|m| m.env_digest.as_ref()),
                )?;
                if let Some(metadata) = metadata {
                    image_defaults = metadata.image_defaults;
                    accepted.network_policy = crate::snapshot::restore_network_policy(
                        metadata.network_policy.as_ref(),
                        accepted.network_policy_request.as_ref(),
                    )?;
                } else {
                    accepted.network_policy =
                        crate::snapshot::restore_network_policy(None, accepted.network_policy_request.as_ref())?;
                }
                let mut bytes = 0u64;
                for path in [files.rootfs(), files.memory(), files.vm_state()] {
                    bytes = bytes
                        .checked_add(tokio::fs::metadata(path).await?.len())
                        .ok_or_else(|| anyhow::anyhow!("local snapshot size overflows"))?;
                }
                let reservation = crate::space::reserve(dir.runtime_dir(), bytes)?;
                *disk_space = Some(Arc::clone(&reservation));
                for (source, destination) in [
                    (files.rootfs(), dir.rootfs()),
                    (files.memory(), dir.runtime_dir().join(crate::snapshot::MEMORY_FILE)),
                    (files.vm_state(), dir.runtime_dir().join(crate::snapshot::STATE_FILE)),
                ] {
                    crate::disk::copy_private_reserved(&source, &destination, Arc::clone(&reservation)).await?;
                }
            } else if let Some(reference) = &accepted.restore {
                let restored = crate::snapshot::pull_into(
                    &self.oci,
                    reference,
                    dir,
                    crate::snapshot::RestoreMachine {
                        memory: accepted.memory,
                        vcpus: self.config.default_vcpus,
                        rootfs_max_bytes: self.config.snapshot_rootfs_max_bytes,
                    },
                    &self.config,
                )
                .await
                .map_err(|e| ProviderError::RootfsCopy(e.to_string()))?;
                *disk_space = Some(restored.space);
                let metadata = restored.metadata;
                image_defaults = metadata.image_defaults;
                accepted.network_policy = crate::snapshot::restore_network_policy(
                    metadata.network_policy.as_ref(),
                    accepted.network_policy_request.as_ref(),
                )?;
            } else if let Some(snapshot) = &warm {
                let info = crate::snapshot::directory_info(snapshot.path()).await?;
                *disk_space = Some(crate::space::reserve(dir.runtime_dir(), info.bytes)?);
                crate::warm_pool::restore(snapshot, dir)
                    .await
                    .map_err(|e| ProviderError::RootfsCopy(e.to_string()))?;
            } else {
                let source = base
                    .as_ref()
                    .map(|base| base.path.as_path())
                    .unwrap_or(self.config.base_rootfs.as_path());
                let source_bytes = tokio::fs::metadata(source)
                    .await
                    .map_err(|error| ProviderError::RootfsCopy(error.to_string()))?
                    .len();
                *disk_space = Some(crate::space::reserve(dir.runtime_dir(), source_bytes)?);
                crate::disk::copy_private(source, &dir.rootfs())
                    .await
                    .map_err(|e| ProviderError::RootfsCopy(e.to_string()))?;
            }
            if let Some(policy) = &accepted.network_policy {
                if !self
                    .profile()
                    .satisfies(&trogon_sandbox_contract::CapabilityRequirements::default().with_network_policy(policy))
                {
                    return Err(anyhow::anyhow!(
                        "snapshot network policy requires unavailable worker capabilities"
                    ));
                }
                anyhow::ensure!(
                    self.config.legacy_network.is_none() || accepted.restore.is_none(),
                    "networked snapshot restore requires host-links mode"
                );
                let (credentials, forward) = crate::lease_credentials::for_lease(
                    &self.config,
                    self.committed_fence.as_ref(),
                    &accepted.tenant,
                    lease_id,
                    policy,
                )
                .await?;
                *network = Some(match (&self.config.network, &self.config.legacy_network) {
                    (Some(config), None) => {
                        crate::network::LeaseNetwork::prepare(
                            config,
                            self.config.max_concurrent_leases,
                            dir,
                            policy.clone(),
                            credentials,
                            forward,
                        )
                        .await?
                    }
                    (None, Some(config)) => {
                        crate::network::LeaseNetwork::start(config, dir, lease_id, policy, credentials).await?
                    }
                    _ => {
                        return Err(anyhow::anyhow!("network configuration is missing or ambiguous"));
                    }
                });
            }
            if let Some(policy) = &accepted.snapshot_out {
                let disk_bytes = tokio::fs::metadata(dir.rootfs()).await?.len();
                let reserved = disk_bytes
                    .checked_add(u64::from(accepted.memory.get()) * 1024 * 1024)
                    .and_then(|bytes| bytes.checked_add(32 * 1024 * 1024))
                    .ok_or_else(|| anyhow::anyhow!("snapshot reservation size overflows"))?;
                // A no-TTL lease has no deadline, so its reservation is kept
                // until the lease is released and the snapshot retained.
                let deadline = std::time::SystemTime::now()
                    .checked_add(lifetime.unwrap_or(NO_TTL_RESERVATION_SPAN))
                    .ok_or_else(|| anyhow::anyhow!("lease expiry overflows"))?;
                self.depot().await?.arm_owned(
                    lease_id,
                    policy,
                    reserved,
                    deadline,
                    self.resources.directory_receipt(dir).await?.as_ref(),
                )?;
            }
            Ok(())
        })
        .await
        .map_err(downcast)?;

        if let Some(image) = &image_defaults {
            let mut env = image.env().clone();
            env.extend(accepted.env);
            accepted.env = env;
            if !accepted.working_dir_explicit
                && let Some(working_dir) = image.working_dir()
            {
                accepted.working_dir = working_dir.to_owned();
            }
        }

        let phase = if accepted.restore.is_some() || warm.is_some() {
            SandboxPhase::RestoreSnapshot
        } else {
            SandboxPhase::BootRuntime
        };
        let agent = run_phase(emitter, phase, None, || async {
            let launched = self
                .launcher
                .launch_with_network(dir, accepted.memory, self.config.default_vcpus, network.as_ref())
                .await?;
            let running = process.insert(launched);
            running
                .wait_for_api_socket(&dir.api_socket(), self.config.boot_timeout)
                .await?;
            let api = FirecrackerApi::new(dir.api_socket());
            if accepted.restore.is_some() || warm.is_some() {
                api.load_snapshot_files(crate::snapshot::STATE_FILE.into(), crate::snapshot::MEMORY_FILE.into())
                    .await
                    .map_err(ProviderError::from)?;
            } else {
                let actor_id = if warm_enabled {
                    LeaseId::new("warm-template")
                } else {
                    lease_id.clone()
                };
                let mut runtime_config = self.config.as_ref().clone();
                runtime_config.kernel = self.launcher.kernel_path(&runtime_config.kernel);
                if base.is_some() {
                    runtime_config.boot_args.push_str(&format!(
                        " root=/dev/vda rw init=/.sandbox/init sandbox.agent_port={}",
                        self.config.agent_port.get()
                    ));
                }
                if network.is_some() {
                    runtime_config.boot_args.push_str(" net.ifnames=0");
                }
                api.configure_and_start(&runtime_config, accepted.memory, &actor_id, network.is_some())
                    .await
                    .map_err(ProviderError::from)?;
            }
            let agent = GuestAgent::new(VsockEndpoint::new(dir.vsock_socket(), self.config.agent_port))
                .with_env(&trogon_std::env::SystemEnv);
            agent
                .wait_ready(self.config.boot_timeout)
                .await
                .map_err(ProviderError::from)?;
            if warm_enabled && accepted.restore.is_none() && warm.is_none() {
                let mut metadata = crate::snapshot::SnapshotMetadata::new(accepted.memory, self.config.default_vcpus)
                    .with_env_digest(self.config.environment.own.clone());
                metadata.image_defaults = image_defaults.clone();
                metadata.network_policy = accepted.network_policy.clone();
                let snapshot = crate::snapshot::LocalSnapshot::capture(dir, metadata).await?;
                self.warm_pool.lock().await.insert(warm_key, snapshot).await?;
            }
            Ok::<_, anyhow::Error>(agent)
        })
        .await
        .map_err(downcast)?;

        run_phase(emitter, SandboxPhase::PrepareWorkspace, None, || async {
            let env = match network.as_ref() {
                Some(network) => network.guest_env_with(&accepted.env)?,
                None => accepted.env.clone(),
            };
            let entropy_seed = if accepted.restore.is_some() || warm_enabled {
                let mut bytes = [0u8; 32];
                getrandom::fill(&mut bytes).map_err(|error| ProviderError::Launch(format!("host entropy: {error}")))?;
                Some(GuestEntropySeed::new(bytes))
            } else {
                None
            };
            agent
                .bind(&BindLeaseRequest {
                    entropy_seed,
                    lease_id: lease_id.clone(),
                    env,
                    working_dir: Some(accepted.working_dir.clone()),
                    exposed_port: None,
                })
                .await
                .map_err(ProviderError::from)?;
            ensure_working_dir(&agent, lease_id, &accepted.working_dir).await?;
            if let Some(network) = network.as_ref() {
                network.configure_guest(&agent, lease_id).await?;
            }
            if accepted.restore.is_some() || warm_enabled {
                refresh_guest_identity(&agent, lease_id).await?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(downcast)?;

        Ok(BootedVm { agent, image_defaults })
    }

    /// Stops the VM and deletes its directory. Safe to call for a lease that is unknown, was
    /// already cancelled, or is still being torn down.
    pub async fn cancel_inner(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let _gate = self.lease_gates.lock(lease_id).await;
        self.cancel_locked(lease_id).await
    }

    async fn cancel_locked(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.losses.lock().await.remove(lease_id.as_str());
        let has_policy = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .is_some_and(|vm| vm.snapshot_policy.is_some());
        if has_policy
            || self.retained.get().is_some()
            || self
                .config
                .state_dir
                .as_path()
                .join(crate::retained::DIRECTORY)
                .exists()
        {
            let depot = self.depot().await.map_err(downcast)?;
            if let Err(error) = self.retain_armed(lease_id, depot).await
                && !matches!(
                    error.downcast_ref::<crate::retained::ReleaseExportError>(),
                    Some(crate::retained::ReleaseExportError::Failed(_))
                )
            {
                return Err(downcast(error));
            }
        }
        self.remove_active(lease_id).await
    }

    async fn purge_inner(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let _gate = self.lease_gates.lock(lease_id).await;
        self.purge_locked(lease_id).await
    }

    async fn purge_locked(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let depot = if self.retained.get().is_some()
            || self
                .config
                .state_dir
                .as_path()
                .join(crate::retained::DIRECTORY)
                .exists()
        {
            Some(self.depot().await.map_err(downcast)?)
        } else {
            None
        };
        if let Some(depot) = depot {
            depot.mark_purged(lease_id).await.map_err(downcast)?;
        }
        self.remove_active(lease_id).await?;
        if let Some(depot) = depot {
            depot.delete_purged(lease_id).map_err(downcast)?;
        }
        Ok(())
    }

    async fn remove_active(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let Some(mut vm) = self.active.lock().await.remove(lease_id.as_str()) else {
            return Ok(());
        };
        vm.process.stop().await;
        if let Some(network) = vm.network.as_mut()
            && let Err(error) = network.shutdown(&vm.dir).await
        {
            self.active.lock().await.insert(lease_id.as_str().to_owned(), vm);
            return Err(ProviderError::Launch(error.to_string()));
        }
        if let Err(error) = self.launcher.cleanup(&vm.dir).await {
            self.active.lock().await.insert(lease_id.as_str().to_owned(), vm);
            return Err(error);
        }
        if let Err(error) = vm.dir.remove().await {
            self.active.lock().await.insert(lease_id.as_str().to_owned(), vm);
            return Err(error.into());
        }
        if let Some(resource) = &vm.resource
            && let Err(error) = self.resources.complete(resource).await
        {
            self.active.lock().await.insert(lease_id.as_str().to_owned(), vm);
            return Err(ProviderError::Launch(error.to_string()));
        }
        info!(lease_id = %lease_id.as_str(), "microVM released");
        Ok(())
    }
}

#[async_trait]
impl Provider for FirecrackerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Firecracker
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderFailure> {
        self.lease_inner(req, lease_id)
            .await
            .map_err(|e| ProviderFailure::backend(e.to_string()))
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderFailure> {
        self.cancel_inner(lease_id)
            .await
            .map_err(|e| ProviderFailure::backend(e.to_string()))
    }

    async fn purge(&self, lease_id: &LeaseId) -> Result<(), ProviderFailure> {
        self.purge_inner(lease_id)
            .await
            .map_err(|e| ProviderFailure::backend(e.to_string()))
    }

    async fn snapshot(
        &self,
        lease_id: &LeaseId,
        target: &trogon_sandbox_contract::OciArtifactRef,
    ) -> Result<trogon_sandbox_contract::SnapshotReceipt, ProviderFailure> {
        self.snapshot_inner(lease_id, target)
            .await
            .map_err(|e| ProviderFailure::backend(e.to_string()))
    }

    async fn checkpoint_running(
        &self,
        lease_id: &LeaseId,
        target: &trogon_sandbox_contract::OciArtifactRef,
    ) -> Result<trogon_sandbox_contract::SnapshotReceipt, ProviderFailure> {
        self.checkpoint_to(
            lease_id,
            &trogon_sandbox_contract::SnapshotPushTarget {
                registry: target.registry.clone(),
                repository: target.repository.clone(),
                artifact_type: target.artifact_type,
            },
        )
        .await
        .map_err(|e| ProviderFailure::backend(e.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod main_contract_tests;
