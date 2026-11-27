use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, OciArtifactRef, ProviderKind, ProviderLease, SnapshotReceipt};
use trogon_sandbox_protocol::{Provider, ProviderError, reject_unsupported_privileged};

use crate::KubeVirtProvider;

#[async_trait]
impl Provider for KubeVirtProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::KubeVirt
    }

    async fn lease(&self, request: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        reject_unsupported_privileged(&request.spec, &self.profile)?;
        self.lease_inner(request, lease_id)
            .await
            .map_err(ProviderError::backend)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        self.cancel_locked(lease_id).await.map_err(ProviderError::backend)
    }

    async fn snapshot(&self, lease_id: &LeaseId, target: &OciArtifactRef) -> Result<SnapshotReceipt, ProviderError> {
        self.snapshot_inner(lease_id, target)
            .await
            .map_err(ProviderError::backend)
    }

    async fn checkpoint_running(
        &self,
        lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, ProviderError> {
        self.checkpoint_inner(lease_id, target)
            .await
            .map_err(ProviderError::backend)
    }
}
