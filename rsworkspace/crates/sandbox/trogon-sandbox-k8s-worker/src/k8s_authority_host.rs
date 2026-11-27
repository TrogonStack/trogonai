use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, FreezeOutcome, HeldLease, PurgeOutcome, RedispatchGuard, ThawOutcome,
};

use crate::{K8sTerminals, K8sWorkerProvider};

/// Lets the authority guard inspect, and purge, the leases this worker holds in the cluster.
pub struct K8sAuthorityHost {
    provider: Arc<K8sWorkerProvider>,
    grants: Arc<RedispatchGuard>,
    terminals: Arc<K8sTerminals>,
}

impl K8sAuthorityHost {
    pub fn new(provider: Arc<K8sWorkerProvider>, grants: Arc<RedispatchGuard>, terminals: Arc<K8sTerminals>) -> Self {
        Self {
            provider,
            grants,
            terminals,
        }
    }
}

fn failed(error: crate::K8sWorkerError) -> AuthorityHostError {
    AuthorityHostError::Failed(Box::new(error))
}

#[async_trait]
impl AuthorityHost for K8sAuthorityHost {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        self.provider.authority_held_leases().await.map_err(failed)
    }

    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        self.provider.authority_freeze(lease).await.map_err(failed)
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        self.provider.authority_thaw(lease).await.map_err(failed)
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        self.provider
            .authority_purge(lease, || async {
                self.terminals.close_lease(lease.lease_id()).await;
                self.grants.forget(lease.lease_id());
            })
            .await
            .map_err(failed)
    }
}
