//! A provider double that answers every lease with a fixed outcome.

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderKind, ProviderLease};
use trogon_sandbox_protocol::{Provider, ProviderError};

/// What the double answers with.
pub(super) enum MockOutcome {
    Grant(ProviderLease),
    Fail,
}

pub(super) struct MockProvider {
    pub(super) outcome: MockOutcome,
}

#[async_trait]
impl Provider for MockProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LocalDocker
    }

    async fn lease(&self, _req: &LeaseRequest, _lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        match &self.outcome {
            MockOutcome::Grant(lease) => Ok(lease.clone()),
            MockOutcome::Fail => Err(ProviderError::backend("docker unavailable")),
        }
    }

    async fn cancel(&self, _lease_id: &LeaseId) -> Result<(), ProviderError> {
        Ok(())
    }
}
