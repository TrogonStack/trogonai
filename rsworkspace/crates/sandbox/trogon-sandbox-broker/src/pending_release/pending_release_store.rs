use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;

use super::{PendingRelease, PendingReleaseError};

#[async_trait]
pub trait PendingReleaseStore: Send + Sync {
    /// Stores `release` unless a marker for the lease exists. Returns the
    /// marker now stored and whether this call created it. Concurrent
    /// releases of one lease therefore share one `recorded_at`, which keeps
    /// their refunds on a single quota record.
    async fn create(&self, release: &PendingRelease) -> Result<(PendingRelease, bool), PendingReleaseError>;
    async fn clear(&self, lease_id: &LeaseId) -> Result<(), PendingReleaseError>;
    async fn all(&self) -> Result<Vec<PendingRelease>, PendingReleaseError>;
}
