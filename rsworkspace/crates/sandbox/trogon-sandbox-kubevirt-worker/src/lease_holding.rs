use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;

/// Whether this worker holds a lease. The worker's lease state implements it.
#[async_trait]
pub trait LeaseHolding: Send + Sync {
    /// Whether this worker holds `lease_id`.
    async fn holds(&self, lease_id: &LeaseId) -> bool;
}
