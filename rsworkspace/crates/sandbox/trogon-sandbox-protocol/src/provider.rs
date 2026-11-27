use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, OciArtifactRef, ProviderKind, ProviderLease, SnapshotReceipt};

use crate::provider_error::ProviderError;
use crate::snapshot_error::SnapshotError;

/// A sandbox backend a worker drives on behalf of the broker.
#[async_trait]
pub trait Provider: Send + Sync {
    /// The kind of backend this is.
    fn kind(&self) -> ProviderKind;

    /// Provisions a sandbox for `req` under `lease_id`.
    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError>;

    /// Ends the lease.
    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError>;

    /// Ends the lease and discards anything kept so it could be resumed. Providers that
    /// keep nothing past a cancel have nothing more to discard.
    async fn purge(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.cancel(lease_id).await
    }

    /// Exports the sandbox's state to `target`. Providers without snapshot support refuse.
    async fn snapshot(&self, _lease_id: &LeaseId, _target: &OciArtifactRef) -> Result<SnapshotReceipt, ProviderError> {
        Err(ProviderError::Snapshot(SnapshotError::Unsupported))
    }

    /// Live checkpoint of a **running** sandbox without stopping it first.
    ///
    /// Providers that distinguish online vs offline snapshot export should override
    /// this instead of [`Provider::snapshot`], which may stop the sandbox when online
    /// snapshot is unsupported.
    async fn checkpoint_running(
        &self,
        lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, ProviderError> {
        self.snapshot(lease_id, target).await
    }
}
