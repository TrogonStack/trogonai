use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderKind, ProviderLease};
use trogon_sandbox_protocol::{Provider, ProviderError};

use crate::SpaceProvider;

#[async_trait]
impl Provider for SpaceProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Space
    }

    async fn lease(&self, request: &LeaseRequest, lease: &LeaseId) -> Result<ProviderLease, ProviderError> {
        self.lease_inner(request, lease, None)
            .await
            .map_err(ProviderError::backend)
    }

    async fn cancel(&self, lease: &LeaseId) -> Result<(), ProviderError> {
        self.cancel_lease(lease).await.map_err(ProviderError::backend)
    }
}
