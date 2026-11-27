use async_trait::async_trait;
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, OciArtifactRef, ProviderKind, ProviderLease, SnapshotPushTarget, SnapshotReceipt,
};
use trogon_sandbox_protocol::{Provider, ProviderError};

use crate::K8sWorkerProvider;

#[async_trait]
impl Provider for K8sWorkerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::K8sMicroVm
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        self.lease_locked(req, lease_id).await.map_err(ProviderError::backend)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        self.cancel_locked(lease_id).await.map_err(ProviderError::backend)
    }

    async fn snapshot(&self, lease_id: &LeaseId, target: &OciArtifactRef) -> Result<SnapshotReceipt, ProviderError> {
        let push_target = SnapshotPushTarget {
            registry: target.registry.clone(),
            repository: target.repository.clone(),
            artifact_type: target.artifact_type,
        };
        self.checkpoint_for_lease(lease_id, &push_target)
            .await
            .map_err(ProviderError::backend)
    }
}
