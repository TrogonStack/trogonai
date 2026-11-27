use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{ProviderKind, WorkerId};

use super::{WorkerFence, WorkerFenceStore, WorkerPlacementState, fence_kv_key};

#[derive(Default)]
pub struct InMemoryWorkerFenceStore {
    fences: Mutex<HashMap<String, WorkerFence>>,
}

impl InMemoryWorkerFenceStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl WorkerFenceStore for InMemoryWorkerFenceStore {
    async fn get(&self, provider: ProviderKind, worker_id: &WorkerId) -> anyhow::Result<Option<WorkerFence>> {
        Ok(self
            .fences
            .lock()
            .await
            .get(&fence_kv_key(provider, worker_id))
            .cloned())
    }

    async fn set(&self, fence: &WorkerFence) -> anyhow::Result<()> {
        let key = fence_kv_key(fence.provider, &fence.worker_id);
        let mut fences = self.fences.lock().await;
        if fence.state == WorkerPlacementState::Active {
            fences.remove(&key);
        } else {
            fences.insert(key, fence.clone());
        }
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<WorkerFence>> {
        Ok(self.fences.lock().await.values().cloned().collect())
    }
}
