use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, ProviderKind, ProviderLease, SandboxId};
use trogon_sandbox_protocol::subjects::evidence_subject;
use trogon_sandbox_protocol::{Provider, ProviderError};

/// A provider that counts launches, so a repeat dispatch can be told from a second sandbox. Launch `n` (from 0)
/// grants `sandbox-{n}`.
pub struct CountingProvider {
    kind: ProviderKind,
    launches: AtomicU32,
}

impl CountingProvider {
    pub fn new(kind: ProviderKind) -> Arc<Self> {
        Arc::new(Self {
            kind,
            launches: AtomicU32::new(0),
        })
    }

    /// How many sandboxes it has launched.
    pub fn launches(&self) -> u32 {
        self.launches.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Provider for CountingProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    async fn lease(&self, _req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        let n = self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderLease {
            sandbox_id: SandboxId::new(format!("sandbox-{n}")).map_err(ProviderError::backend)?,
            evidence_subject: evidence_subject(lease_id),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        })
    }

    async fn cancel(&self, _lease_id: &LeaseId) -> Result<(), ProviderError> {
        Ok(())
    }
}
