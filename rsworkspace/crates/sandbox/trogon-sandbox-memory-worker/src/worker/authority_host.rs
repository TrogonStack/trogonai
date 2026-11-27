use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_protocol::{AuthorityHost, AuthorityHostError, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome};

use super::state::State;

/// Lets the authority guard reach the leases a worker holds.
pub(crate) struct MemoryAuthorityHost {
    pub state: Arc<State>,
    pub client: async_nats::Client,
}

#[async_trait]
impl AuthorityHost for MemoryAuthorityHost {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self.state.held_leases())
    }

    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        Ok(self.state.freeze(lease).await)
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        Ok(match self.state.freeze(lease).await {
            FreezeOutcome::Stale => ThawOutcome::Stale,
            _ => ThawOutcome::NoLongerOwned,
        })
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        Ok(self.state.purge(&self.client, lease).await)
    }
}
