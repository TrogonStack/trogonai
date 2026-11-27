use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError};

use trogon_sandbox_contract::{ArtifactType, LeaseId, OciArtifactRef, SandboxId, SnapshotReceipt, WorkerId};
use trogon_sandbox_oci::OciClient;
use trogon_sandbox_protocol::AuthorityResourceId;

use crate::runtime_state::RuntimeState;
use crate::sandbox_source::SandboxSource;
use crate::{
    HyperlightError, Hypervisor, LeaseGates, RuntimeError, pack_layout_dir, pull_snapshot_blob, push_snapshot_artifact,
    unpack_layout_dir,
};

/// What the provider shares across leases: the component root, the registry client, the guests
/// the leases hold and the warm runtimes waiting for the next one.
pub struct HyperlightRuntime<H: Hypervisor> {
    component_root: PathBuf,
    oci_client: Arc<OciClient>,
    default_registry: String,
    max_warm_sandboxes: u32,
    worker_id: OnceLock<WorkerId>,
    gates: LeaseGates,
    state: Arc<RuntimeState<H>>,
}

async fn blocking<T, F>(work: F) -> Result<T, HyperlightError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, HyperlightError> + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.map_err(HyperlightError::Join)?
}

impl<H: Hypervisor> HyperlightRuntime<H> {
    /// A runtime over `hypervisor` that loads components from `component_root` and keeps up to
    /// `max_warm_sandboxes` warm runtimes.
    ///
    /// Fails when the root is not a directory or the host has no hypervisor.
    pub fn new(
        hypervisor: H,
        component_root: PathBuf,
        max_warm_sandboxes: u32,
        oci_client: Arc<OciClient>,
        default_registry: String,
    ) -> Result<Self, RuntimeError> {
        if !component_root.is_dir() {
            return Err(RuntimeError::ComponentRoot(component_root));
        }
        if !hypervisor.is_available() {
            return Err(RuntimeError::NoHypervisor);
        }
        let capacity = usize::try_from(max_warm_sandboxes).unwrap_or(usize::MAX);
        Ok(Self {
            component_root,
            oci_client,
            default_registry,
            max_warm_sandboxes,
            worker_id: OnceLock::new(),
            gates: LeaseGates::default(),
            state: Arc::new(RuntimeState::new(hypervisor, capacity)),
        })
    }

    /// Leases currently holding a guest.
    pub fn active_lease_count(&self) -> usize {
        self.state.leases().len()
    }

    /// The directory component names resolve under.
    pub fn component_root(&self) -> &Path {
        &self.component_root
    }

    /// The most warm runtimes the worker keeps.
    pub fn max_warm_sandboxes(&self) -> u32 {
        self.max_warm_sandboxes
    }

    /// Binds the runtime to `worker`; a runtime serves one worker for its whole life.
    pub fn bind_worker(&self, worker: &WorkerId) -> Result<(), RuntimeError> {
        if self.worker_id.get_or_init(|| worker.clone()) == worker {
            Ok(())
        } else {
            Err(RuntimeError::WorkerBound)
        }
    }

    pub(crate) fn worker_id(&self) -> Option<WorkerId> {
        self.worker_id.get().cloned()
    }

    pub(crate) fn gate(&self, lease: &LeaseId) -> Arc<tokio::sync::Mutex<()>> {
        self.gates.gate(lease)
    }

    pub(crate) fn holds(&self, lease: &LeaseId) -> bool {
        self.state.leases().contains(lease)
    }

    pub(crate) fn inventory(&self) -> Vec<(LeaseId, AuthorityResourceId)> {
        self.state.leases().inventory()
    }

    pub(crate) fn matches(&self, lease: &LeaseId, resource: &AuthorityResourceId) -> bool {
        self.state.leases().matches(lease, resource)
    }

    pub(crate) fn is_completed(&self, lease: &LeaseId, resource: &AuthorityResourceId) -> bool {
        self.state.leases().is_completed(lease, resource)
    }

    /// Loads the guest for `lease` from `source` and retains it.
    pub(crate) async fn acquire(&self, lease: &LeaseId, source: SandboxSource) -> Result<SandboxId, HyperlightError> {
        let sandbox_id = SandboxId::new(format!("hl-{}", lease.as_str()))?;
        let state = Arc::clone(&self.state);
        let guest = match source {
            SandboxSource::Component(component) => blocking(move || Ok(state.open_component(&component)?)).await?,
            SandboxSource::Snapshot(reference) => {
                let blob = pull_snapshot_blob(&self.oci_client, &reference).await?;
                blocking(move || {
                    let scratch = tempfile::tempdir().map_err(HyperlightError::Scratch)?;
                    unpack_layout_dir(&blob, scratch.path())?;
                    Ok(state.open_layout(scratch.path())?)
                })
                .await?
            }
        };
        self.state.retain(lease, guest)?;
        Ok(sandbox_id)
    }

    /// Releases the guest `lease` holds. Returns whether it held one.
    pub(crate) async fn release(&self, lease: &LeaseId, recycle: bool) -> Result<bool, HyperlightError> {
        let state = Arc::clone(&self.state);
        let lease = lease.clone();
        blocking(move || state.release(&lease, recycle)).await
    }

    /// Snapshots the guest of `lease` and publishes it to `target`.
    pub(crate) async fn snapshot(
        &self,
        lease: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, HyperlightError> {
        if target.artifact_type != ArtifactType::HyperlightSnapshotV1 {
            return Err(HyperlightError::PublishArtifactType(target.artifact_type));
        }
        let guest = self
            .state
            .leases()
            .get(lease)
            .ok_or_else(|| HyperlightError::NoLease(lease.as_str().to_owned()))?;
        let state = Arc::clone(&self.state);
        let blob = blocking(move || {
            let mut held = guest.lock().unwrap_or_else(PoisonError::into_inner);
            let scratch = tempfile::tempdir().map_err(HyperlightError::Scratch)?;
            let layout = scratch.path().join("layout");
            state.hypervisor.save_layout(&mut held, &layout)?;
            Ok(pack_layout_dir(&layout)?)
        })
        .await?;
        let registry = if target.registry.is_empty() {
            self.default_registry.as_str()
        } else {
            target.registry.as_str()
        };
        let (reference, bytes) = push_snapshot_artifact(&self.oci_client, registry, &target.repository, blob).await?;
        Ok(SnapshotReceipt {
            digest: reference.digest,
            bytes,
            artifact_type: ArtifactType::HyperlightSnapshotV1,
        })
    }
}
