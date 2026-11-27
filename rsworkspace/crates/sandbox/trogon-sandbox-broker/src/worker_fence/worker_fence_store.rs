use async_trait::async_trait;
use trogon_sandbox_contract::{ProviderKind, WorkerId};

use super::WorkerFence;

#[async_trait]
pub trait WorkerFenceStore: Send + Sync {
    async fn get(&self, provider: ProviderKind, worker_id: &WorkerId) -> anyhow::Result<Option<WorkerFence>>;
    /// Stores `fence`, or removes the record when it is
    /// `WorkerPlacementState::Active` so the bucket only holds fences.
    async fn set(&self, fence: &WorkerFence) -> anyhow::Result<()>;
    async fn list(&self) -> anyhow::Result<Vec<WorkerFence>>;
}
