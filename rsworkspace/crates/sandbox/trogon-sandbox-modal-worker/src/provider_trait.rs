use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderKind, ProviderLease};
use trogon_sandbox_protocol::{Provider, ProviderError, reject_unsupported_privileged};

use crate::ModalProvider;

#[async_trait]
impl Provider for ModalProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Modal
    }

    async fn lease(&self, request: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        reject_unsupported_privileged(&request.spec, &self.profile)?;
        self.lease_inner(request, lease_id)
            .await
            .map_err(ProviderError::backend)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.cancel_inner(lease_id).await.map_err(ProviderError::backend)
    }
}
