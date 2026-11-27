use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, FreezeOutcome, HeldLease, PurgeOutcome, RedispatchGuard, ThawOutcome,
};
use trogon_sandbox_ssh_gateway::TerminalHub;

use crate::WorkerProvider;

/// The provider as the authority guard sees it. A purged lease also loses its terminals and its first grant, so a
/// later dispatch under the same lease id launches a fresh sandbox.
pub struct WorkerAuthorityHost {
    provider: Arc<dyn WorkerProvider>,
    terminals: Arc<TerminalHub>,
    grants: Arc<RedispatchGuard>,
}

impl WorkerAuthorityHost {
    pub fn new(provider: Arc<dyn WorkerProvider>, terminals: Arc<TerminalHub>, grants: Arc<RedispatchGuard>) -> Self {
        Self {
            provider,
            terminals,
            grants,
        }
    }
}

#[async_trait]
impl AuthorityHost for WorkerAuthorityHost {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        self.provider.held_leases().await
    }

    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        self.provider.freeze(lease).await
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        self.provider.thaw(lease).await
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let outcome = AuthorityHost::purge(&*self.provider, lease).await?;
        if outcome != PurgeOutcome::Stale {
            self.terminals.close_lease(lease.lease_id()).await;
            self.grants.forget(lease.lease_id());
        }
        Ok(outcome)
    }
}
