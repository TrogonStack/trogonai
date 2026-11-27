use trogon_sandbox_contract::{ArtifactType, LeaseId, OciArtifactRef, SnapshotReceipt};

use crate::{KubeVirtProvider, KubeVirtProviderError, SNAPSHOT_TENANT};

impl KubeVirtProvider {
    pub(crate) async fn snapshot_inner(
        &self,
        lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, KubeVirtProviderError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let vm_name = self.snapshot_target(lease_id, target).await?;
        let labels = self.ops.lease_labels(lease_id.as_str(), SNAPSHOT_TENANT);
        self.snapshots
            .export(&self.ops, lease_id, &vm_name, labels, target)
            .await
            .map_err(KubeVirtProviderError::Snapshot)
    }

    pub(crate) async fn checkpoint_inner(
        &self,
        lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, KubeVirtProviderError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let vm_name = self.snapshot_target(lease_id, target).await?;
        if !self.ops.vmi_is_running(&vm_name).await? {
            return Err(KubeVirtProviderError::NotRunning {
                lease_id: lease_id.clone(),
            });
        }
        let labels = self.ops.lease_labels(lease_id.as_str(), SNAPSHOT_TENANT);
        let receipt = self
            .snapshots
            .checkpoint(&self.ops, lease_id, &vm_name, labels, target)
            .await
            .map_err(KubeVirtProviderError::Snapshot)?;
        if !self.ops.vmi_is_running(&vm_name).await? {
            return Err(KubeVirtProviderError::StoppedDuringCheckpoint {
                vm_name,
                lease_id: lease_id.clone(),
            });
        }
        Ok(receipt)
    }

    /// The VM an export of the lease reads, once the target is known to be a KubeVirt disk.
    async fn snapshot_target(
        &self,
        lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<String, KubeVirtProviderError> {
        if target.artifact_type != ArtifactType::KubevirtDiskV1 {
            return Err(KubeVirtProviderError::ArtifactType {
                actual: target.artifact_type,
            });
        }
        let active = self.active.lock().await;
        let held = active.get(lease_id);
        held.map(|held| held.vm_name.clone())
            .ok_or_else(|| KubeVirtProviderError::NoActiveLease {
                lease_id: lease_id.clone(),
            })
    }
}
