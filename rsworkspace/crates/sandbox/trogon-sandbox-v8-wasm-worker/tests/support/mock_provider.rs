use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderKind, ProviderLease, SandboxId};
use trogon_sandbox_protocol::subjects::evidence_subject;
use trogon_sandbox_protocol::{Provider, ProviderError};

/// Tenant whose leases always fail.
pub const FAILING_TENANT: &str = "fail-me";
/// Lease whose cancel always fails.
pub const UNCANCELLABLE_LEASE: &str = "cannot-cancel";

/// A provider that only records what it was asked to do.
#[derive(Default)]
pub struct MockProvider {
    leased: Mutex<Vec<String>>,
    cancelled: Mutex<Vec<String>>,
}

impl MockProvider {
    pub fn leased(&self) -> Vec<String> {
        self.leased.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn cancelled(&self) -> Vec<String> {
        self.cancelled.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::V8Wasm
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        self.leased
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(lease_id.as_str().to_owned());
        if req.tenant.as_str() == FAILING_TENANT {
            return Err(ProviderError::backend("synthetic provider failure"));
        }
        Ok(ProviderLease {
            sandbox_id: SandboxId::new(lease_id.as_str()).map_err(ProviderError::backend)?,
            evidence_subject: evidence_subject(lease_id),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        })
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.cancelled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(lease_id.as_str().to_owned());
        (lease_id.as_str() != UNCANCELLABLE_LEASE)
            .then_some(())
            .ok_or_else(|| ProviderError::backend("synthetic cancel failure"))
    }
}
