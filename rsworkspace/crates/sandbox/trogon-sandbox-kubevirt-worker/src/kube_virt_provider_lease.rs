use std::collections::BTreeMap;

use tracing::{info, warn};
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, OciArtifactRef, ProviderLease, SandboxId, SandboxImage, SandboxSpec, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects::evidence_subject;

use crate::active_lease::ActiveLease;
use crate::locked::locked;
use crate::provision_receipt::ProvisionReceipt;
use crate::registry_source::registry_source;
use crate::{CustodyPhase, KubeVirtOps, KubeVirtProvider, KubeVirtProviderError, LeaseJournal, MIN_COLD_MEMORY_MIB};

impl KubeVirtProvider {
    pub(crate) async fn lease_inner(
        &self,
        request: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> Result<ProviderLease, KubeVirtProviderError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        self.refuse_while_held(lease_id).await?;
        let journal = LeaseJournal::begin(&self.journal, lease_id, &request.tenant)?;
        let ops = self
            .ops
            .with_durable_receipts(journal.clone(), crate::LeaseResources::default());
        let receipts = ops.receipt_log()?;
        locked(&self.preparing).insert(lease_id.clone());
        let provision = ProvisionReceipt {
            lease: lease_id.clone(),
            preparing: self.preparing.clone(),
            journal: journal.clone(),
            resources: receipts.clone(),
        };
        locked(&self.in_flight).insert(lease_id.clone(), receipts);
        let allocated = self.allocate(request, lease_id, &ops).await;
        let result = match allocated {
            Ok(lease) => journal
                .set_phase(CustodyPhase::Ready)
                .map(|()| lease)
                .map_err(Into::into),
            Err(error) => Err(error),
        };
        drop(provision);
        if result.is_err() {
            self.abandon(lease_id, &ops, &journal).await;
        }
        locked(&self.in_flight).remove(lease_id);
        result
    }

    async fn refuse_while_held(&self, lease_id: &LeaseId) -> Result<(), KubeVirtProviderError> {
        if self.active.lock().await.contains_key(lease_id) {
            return Err(KubeVirtProviderError::LeaseHeld);
        }
        if self.retained.lock().await.contains_key(lease_id) {
            return Err(KubeVirtProviderError::CleanupRetained);
        }
        if self.in_flight_resources(lease_id).is_some() {
            return Err(KubeVirtProviderError::AllocationInterrupted);
        }
        Ok(())
    }

    /// Deletes what a failed allocation created, keeping what cannot be deleted for a later cancel.
    async fn abandon(&self, lease_id: &LeaseId, ops: &KubeVirtOps, journal: &LeaseJournal) {
        let resources = ops.resource_receipts();
        if !resources.is_empty()
            && let Err(error) = ops.release_resources(&resources).await
        {
            self.retained
                .lock()
                .await
                .insert(lease_id.clone(), ops.resource_receipts());
            warn!(lease_id = %lease_id, %error, "retaining failed KubeVirt allocation cleanup");
        }
        self.active.lock().await.remove(lease_id);
        let settled = journal
            .record()
            .is_ok_and(|record| record.resources.is_empty() && record.create.is_none());
        if settled {
            let _ = journal
                .finish()
                .inspect_err(|error| warn!(lease_id = %lease_id, %error, "custody record stays"));
        }
    }

    async fn allocate(
        &self,
        request: &LeaseRequest,
        lease_id: &LeaseId,
        ops: &KubeVirtOps,
    ) -> Result<ProviderLease, KubeVirtProviderError> {
        let spec = &request.spec;
        check_workspace(spec.workspace())?;
        reject_env(spec)?;

        // Warm VMs boot before any lease names a CPU size, so a sized lease cold starts rather than run on
        // whatever cores the pool picked.
        if let SandboxImage::Snapshot { reference, .. } = spec.image()
            && spec.vcpu_millis().is_none()
            && let Some(lease) = self.try_warm_pool_lease(ops, request, lease_id, reference).await?
        {
            return Ok(lease);
        }

        let vm_name = ops.vm_name(lease_id.as_str());
        let dv_name = ops.dv_name(lease_id.as_str());
        let labels = ops.lease_labels(lease_id.as_str(), request.tenant.as_str());
        self.provision_data_volume(ops, spec, &dv_name, labels.clone()).await?;

        let cloud_init = ops.build_cloud_init_user_data(&[]);
        let memory_mib = spec.memory_mib().get().max(MIN_COLD_MEMORY_MIB);
        let vm = ops.build_virtual_machine(&vm_name, &dv_name, memory_mib, spec.vcpu_millis(), &cloud_init, labels);
        let created = ops.create_virtual_machine_owned(vm).await?;
        ops.validate_virtual_machine(&created)?;
        ops.wait_vmi_ready(&vm_name).await?;
        self.probe(&vm_name).await;
        self.hold(lease_id, ops, &vm_name).await;

        if let SandboxImage::Snapshot { reference, .. } = spec.image() {
            self.warm_pool.spawn_prewarm_after_lease(reference.clone());
        }
        info!(%vm_name, lease_id = %lease_id, "kubevirt lease ready");
        self.granted(lease_id, vm_name)
    }

    async fn provision_data_volume(
        &self,
        ops: &KubeVirtOps,
        spec: &SandboxSpec,
        dv_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<(), KubeVirtProviderError> {
        let data_volume = match spec.image() {
            SandboxImage::Snapshot { reference, .. } => {
                self.restored_data_volume(ops, reference, dv_name, labels).await?
            }
            SandboxImage::Registry { reference, .. } => {
                ops.build_data_volume_spec(dv_name, registry_source(reference), labels)
            }
            SandboxImage::SnapshotTag { .. } => return Err(KubeVirtProviderError::SnapshotTagUnresolved),
            SandboxImage::HyperlightModule { .. } => return Err(KubeVirtProviderError::HyperlightUnsupported),
            SandboxImage::NativeSnapshot { .. } => return Err(KubeVirtProviderError::NativeSnapshotUnsupported),
        };
        ops.create_data_volume_owned(data_volume).await?;
        ops.wait_data_volume_ready(dv_name).await?;
        Ok(())
    }

    async fn restored_data_volume(
        &self,
        ops: &KubeVirtOps,
        reference: &OciArtifactRef,
        dv_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<kube::core::DynamicObject, KubeVirtProviderError> {
        let volume = ops
            .config()
            .volume
            .as_ref()
            .ok_or(KubeVirtProviderError::VolumeUnconfigured)?;
        Ok(self
            .snapshots
            .data_volume(ops, volume, reference, dv_name, labels)
            .await?)
    }

    async fn try_warm_pool_lease(
        &self,
        ops: &KubeVirtOps,
        request: &LeaseRequest,
        lease_id: &LeaseId,
        reference: &OciArtifactRef,
    ) -> Result<Option<ProviderLease>, KubeVirtProviderError> {
        let claim = self
            .warm_pool
            .try_claim_with_receipts(
                ops,
                &reference.digest,
                request.spec.memory_mib().get(),
                lease_id.as_str(),
                request.tenant.as_str(),
            )
            .await?;
        let Some(claim) = claim else {
            return Ok(None);
        };
        ops.wait_vmi_ready(&claim.vm_name).await?;
        self.probe(&claim.vm_name).await;
        self.hold(lease_id, ops, &claim.vm_name).await;
        info!(vm_name = %claim.vm_name, lease_id = %lease_id, digest = %reference.digest, "kubevirt warm pool lease ready");
        self.warm_pool.spawn_prewarm_after_lease(reference.clone());
        Ok(Some(self.granted(lease_id, claim.vm_name)?))
    }

    /// A probe that fails only logs: the VMI is ready, so the lease still holds.
    async fn probe(&self, vm_name: &str) {
        let namespace = self.ops.config().namespace.as_str();
        if let Err(error) = self.guest_probe.ready(namespace, vm_name).await {
            warn!(%error, %vm_name, "post-boot exec probe failed (VMI is Ready)");
        }
    }

    async fn hold(&self, lease_id: &LeaseId, ops: &KubeVirtOps, vm_name: &str) {
        let active = ActiveLease {
            resources: ops.resource_receipts(),
            vm_name: vm_name.to_owned(),
        };
        self.active.lock().await.insert(lease_id.clone(), active);
    }

    fn granted(&self, lease_id: &LeaseId, vm_name: String) -> Result<ProviderLease, KubeVirtProviderError> {
        Ok(ProviderLease {
            sandbox_id: SandboxId::new(vm_name)?,
            evidence_subject: evidence_subject(lease_id),
            worker_id: Some(self.worker_id().clone()),
            attempt_id: None,
            epoch: None,
        })
    }
}

/// Only an empty workspace is supported: the VM never imports or mounts one, so any other source would be accepted
/// and silently dropped.
fn check_workspace(workspace: &WorkspaceSource) -> Result<(), KubeVirtProviderError> {
    let name = match workspace {
        WorkspaceSource::Empty(_) => return Ok(()),
        WorkspaceSource::Clone { .. } => "Clone",
        WorkspaceSource::HostMount { .. } => "HostMount",
        WorkspaceSource::S3Volume { .. } => "S3Volume",
    };
    Err(KubeVirtProviderError::WorkspaceUnsupported { workspace: name })
}

/// The VM boots from its image with no channel for `spec.env`, so a non-empty one is refused rather than silently
/// dropped.
fn reject_env(spec: &SandboxSpec) -> Result<(), KubeVirtProviderError> {
    if spec.env().is_empty() {
        Ok(())
    } else {
        Err(KubeVirtProviderError::EnvUnsupported)
    }
}
