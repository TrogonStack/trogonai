use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderKind, ProviderLease};
use trogon_sandbox_protocol::{Provider, ProviderError};

use super::E2bWorkerProvider;

#[async_trait]
impl Provider for E2bWorkerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::E2b
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        Ok(self.allocate_lease(req, lease_id, None).await?)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        Ok(self.cancel_lease(lease_id).await?)
    }
}
