use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;

use super::{PendingPurge, PendingPurgeError};

#[async_trait]
pub trait PendingPurgeStore: Send + Sync {
    async fn put(&self, purge: &PendingPurge) -> Result<(), PendingPurgeError>;
    async fn get(&self, lease_id: &LeaseId) -> Result<Option<PendingPurge>, PendingPurgeError>;
    async fn clear(&self, lease_id: &LeaseId) -> Result<(), PendingPurgeError>;
    async fn all(&self) -> Result<Vec<PendingPurge>, PendingPurgeError>;
}
