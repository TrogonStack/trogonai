//! Authority cleanup targets the exact owned child task. Raw PID signals
//! cannot prove incarnation identity, so this host has no freeze primitive.

use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_protocol::{AuthorityHost, AuthorityHostError, HeldLease, PurgeOutcome};

use crate::subprocess::SubprocessWorker;

pub struct SubprocessAuthorityHost {
    worker: Arc<SubprocessWorker>,
}

impl SubprocessAuthorityHost {
    pub fn new(worker: Arc<SubprocessWorker>) -> Self {
        Self { worker }
    }
}

#[async_trait]
impl AuthorityHost for SubprocessAuthorityHost {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        self.worker.authority_held_leases().await
    }
    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        self.worker.authority_purge(lease).await
    }
}
