use std::collections::HashMap;
use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, LeaseId, LeaseRequest, OciArtifactRef, ProviderCapabilityProfile, ProviderKind,
    ProviderLease, SandboxImage, SnapshotReceipt, WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol,
    WorkerHeartbeat, WorkerId, WorkspaceSource,
};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, FreezeOutcome, HeldLease, Provider, ProviderError, PurgeOutcome, ThawOutcome,
    reject_unsupported_privileged, subjects,
};

use crate::sandbox_source::SandboxSource;
use crate::{HyperlightError, HyperlightRuntime, Hypervisor, RuntimeError, capability_profile, resolve_component_path};

impl From<HyperlightError> for ProviderError {
    fn from(error: HyperlightError) -> Self {
        Self::backend(error)
    }
}

/// The Hyperlight provider: serves leases as Wasm components in hypervisor-isolated guests and
/// answers the authority guard for the guests it holds.
pub struct HyperlightWorkerProvider<H: Hypervisor> {
    runtime: HyperlightRuntime<H>,
    profile: ProviderCapabilityProfile,
    max_concurrent_leases: u32,
}

impl<H: Hypervisor> HyperlightWorkerProvider<H> {
    /// A provider over `runtime` that advertises room for `max_concurrent_leases` leases.
    pub fn new(runtime: HyperlightRuntime<H>, max_concurrent_leases: u32) -> Self {
        let profile = capability_profile(runtime.max_warm_sandboxes(), max_concurrent_leases);
        Self {
            runtime,
            profile,
            max_concurrent_leases,
        }
    }

    /// What this provider advertises.
    pub fn profile(&self) -> &ProviderCapabilityProfile {
        &self.profile
    }

    /// Leases currently holding a guest.
    pub fn active_lease_count(&self) -> usize {
        self.runtime.active_lease_count()
    }

    /// The heartbeat `worker_id` publishes: the leases held now, the room left and the profile.
    pub fn heartbeat(&self, worker_id: &WorkerId) -> WorkerHeartbeat {
        WorkerHeartbeat {
            worker_id: worker_id.clone(),
            dispatch_protocol: WorkerDispatchProtocol::default(),
            native_volume_bindings: Vec::new(),
            provider: ProviderKind::Hyperlight,
            capacity: WorkerCapacity {
                max_concurrent_leases: self.max_concurrent_leases,
                current_leases: u32::try_from(self.active_lease_count()).unwrap_or(u32::MAX),
                capabilities: WorkerCapabilities::from(&self.profile),
            },
            emitted_at: SystemTime::now(),
            cached_digests: Vec::new(),
            profile: self.profile.clone(),
            warm_pool_population: HashMap::new(),
            supports_lease_epoch: CapabilitySupport::default(),
        }
    }

    /// Binds the provider to `worker`.
    pub fn bind_worker(&self, worker: &WorkerId) -> Result<(), RuntimeError> {
        self.runtime.bind_worker(worker)
    }

    fn source(&self, request: &LeaseRequest) -> Result<SandboxSource, HyperlightError> {
        match request.spec.image() {
            SandboxImage::Snapshot { reference, .. } => {
                if reference.artifact_type == ArtifactType::HyperlightSnapshotV1 {
                    Ok(SandboxSource::Snapshot(reference.clone()))
                } else {
                    Err(HyperlightError::RestoreArtifactType(reference.artifact_type))
                }
            }
            SandboxImage::SnapshotTag { artifact_type, .. } => {
                Err(HyperlightError::UnresolvedSnapshotTag(*artifact_type))
            }
            SandboxImage::NativeSnapshot { .. } => Err(HyperlightError::NativeSnapshot),
            image @ (SandboxImage::HyperlightModule { .. } | SandboxImage::Registry { .. }) => Ok(
                SandboxSource::Component(resolve_component_path(self.runtime.component_root(), image)?),
            ),
        }
    }

    fn check(&self, request: &LeaseRequest) -> Result<SandboxSource, HyperlightError> {
        if !request.spec.env().is_empty() {
            return Err(HyperlightError::Env);
        }
        match request.spec.workspace() {
            WorkspaceSource::Empty(_) => {}
            WorkspaceSource::Clone { .. } => return Err(HyperlightError::CloneWorkspace),
            WorkspaceSource::HostMount { .. } => return Err(HyperlightError::HostMountWorkspace),
            WorkspaceSource::S3Volume { .. } => return Err(HyperlightError::S3Workspace),
        }
        self.source(request)
    }

    async fn lease_guarded(&self, request: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        if self.runtime.holds(lease_id) {
            return Err(HyperlightError::Retained.into());
        }
        reject_unsupported_privileged(&request.spec, &self.profile)?;
        let source = self.check(request)?;
        let sandbox_id = self
            .runtime
            .acquire(lease_id, source)
            .await
            .map_err(|error| HyperlightError::Acquire(Box::new(error)))?;
        Ok(ProviderLease {
            sandbox_id,
            evidence_subject: subjects::evidence_subject(lease_id),
            worker_id: self.runtime.worker_id(),
            attempt_id: None,
            epoch: None,
        })
    }
}

#[async_trait]
impl<H: Hypervisor> Provider for HyperlightWorkerProvider<H> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Hyperlight
    }

    async fn lease(&self, request: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        let gate = self.runtime.gate(lease_id);
        let _held = gate.lock().await;
        self.lease_guarded(request, lease_id).await
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let gate = self.runtime.gate(lease_id);
        let _held = gate.lock().await;
        self.runtime.release(lease_id, true).await?;
        Ok(())
    }

    async fn snapshot(&self, lease_id: &LeaseId, target: &OciArtifactRef) -> Result<SnapshotReceipt, ProviderError> {
        Ok(self.runtime.snapshot(lease_id, target).await?)
    }

    async fn checkpoint_running(
        &self,
        lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, ProviderError> {
        self.snapshot(lease_id, target).await
    }
}

#[async_trait]
impl<H: Hypervisor> AuthorityHost for HyperlightWorkerProvider<H> {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self
            .runtime
            .inventory()
            .into_iter()
            .map(|(lease_id, resource_id)| HeldLease::Provider {
                lease_id,
                provider: ProviderKind::Hyperlight,
                resource_id,
            })
            .collect())
    }

    async fn freeze(&self, held: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::Hyperlight,
            resource_id,
        } = held
        else {
            return Ok(FreezeOutcome::Stale);
        };
        Ok(if self.runtime.matches(lease_id, resource_id) {
            FreezeOutcome::Unsupported
        } else {
            FreezeOutcome::Stale
        })
    }

    async fn thaw(&self, held: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::Hyperlight,
            resource_id,
        } = held
        else {
            return Ok(ThawOutcome::Stale);
        };
        Ok(
            if self.runtime.matches(lease_id, resource_id) || self.runtime.is_completed(lease_id, resource_id) {
                ThawOutcome::NoLongerOwned
            } else {
                ThawOutcome::Stale
            },
        )
    }

    async fn purge(&self, held: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::Hyperlight,
            resource_id,
        } = held
        else {
            return Ok(PurgeOutcome::Stale);
        };
        let gate = self.runtime.gate(lease_id);
        let _held = gate.lock().await;
        if self.runtime.is_completed(lease_id, resource_id) {
            return Ok(PurgeOutcome::Settled);
        }
        if !self.runtime.matches(lease_id, resource_id) {
            return Ok(PurgeOutcome::Stale);
        }
        match self.runtime.release(lease_id, false).await {
            Ok(true) => Ok(PurgeOutcome::Purged),
            Ok(false) => Ok(PurgeOutcome::Stale),
            Err(error) => Err(AuthorityHostError::Failed(Box::new(error))),
        }
    }
}
