use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderKind, ProviderLease};
use trogon_sandbox_protocol::{Provider, ProviderError};

use crate::ProxmoxProvider;

#[async_trait]
impl Provider for ProxmoxProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Proxmox
    }

    async fn lease(&self, request: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        self.lease_inner(request, lease_id, None)
            .await
            .map_err(ProviderError::backend)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.release(lease_id).await.map_err(ProviderError::backend)
    }
}
