//! A provider double that counts launches, so a repeat dispatch can be told from a second sandbox.

use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderKind, ProviderLease, SandboxId};
use trogon_sandbox_protocol::{Provider, ProviderError, subjects::evidence_subject};

#[derive(Default)]
pub(super) struct CountingProvider {
    launches: AtomicU32,
}

impl CountingProvider {
    pub(super) fn launches(&self) -> u32 {
        self.launches.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Provider for CountingProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LocalDocker
    }

    async fn lease(&self, _req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        let n = self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new(format!("sandbox-{n}")).map_err(ProviderError::backend)?,
            evidence_subject: evidence_subject(lease_id),
            worker_id: None,
            attempt_id: None,
        })
    }

    async fn cancel(&self, _lease_id: &LeaseId) -> Result<(), ProviderError> {
        Ok(())
    }
}
