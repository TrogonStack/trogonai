use trogon_sandbox_contract::{LeaseId, OciArtifactRef, SnapshotPushTarget, SnapshotReceipt};

use crate::snapshot::push_rootfs_tar;
use crate::{K8sWorkerError, K8sWorkerProvider};

impl K8sWorkerProvider {
    /// Exports `lease_id`'s workspace from its running pod and pushes it to
    /// `target`, leaving the pod running.
    pub async fn checkpoint_for_lease(
        &self,
        lease_id: &LeaseId,
        target: &SnapshotPushTarget,
    ) -> Result<SnapshotReceipt, K8sWorkerError> {
        let pod_name = self
            .sandbox_for_lease(lease_id)
            .await
            .ok_or_else(|| K8sWorkerError::NoActiveLease(lease_id.clone()))?;

        let tar_bytes = self.inner.export_workspace_tar_from_pod(&pod_name).await?;

        let receipt = push_rootfs_tar(self.oci.as_ref(), target, &tar_bytes).await?;
        self.remember_pushed(target, &receipt);
        Ok(receipt)
    }

    /// Records a pushed snapshot so the warm pool can build pods from it.
    pub fn remember_pushed(&self, target: &SnapshotPushTarget, receipt: &SnapshotReceipt) {
        self.digest_cache.insert(receipt.digest.clone());
        if let Some(warm_pool) = &self.warm_pool {
            warm_pool.remember_source(&OciArtifactRef {
                registry: target.registry.clone(),
                repository: target.repository.clone(),
                digest: receipt.digest.clone(),
                artifact_type: receipt.artifact_type,
            });
        }
    }
}
