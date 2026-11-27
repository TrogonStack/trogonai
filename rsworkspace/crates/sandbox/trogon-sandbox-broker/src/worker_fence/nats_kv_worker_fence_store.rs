use async_nats::jetstream::{self, kv};
use async_trait::async_trait;
use futures::StreamExt;
use tracing::warn;
use trogon_sandbox_contract::{ProviderKind, WorkerId};

use super::{WORKER_FENCE_BUCKET, WorkerFence, WorkerFenceStore, WorkerPlacementState, fence_kv_key};

pub struct NatsKvWorkerFenceStore {
    pub kv: kv::Store,
}

impl NatsKvWorkerFenceStore {
    pub async fn new(js: jetstream::Context) -> anyhow::Result<Self> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: WORKER_FENCE_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl WorkerFenceStore for NatsKvWorkerFenceStore {
    async fn get(&self, provider: ProviderKind, worker_id: &WorkerId) -> anyhow::Result<Option<WorkerFence>> {
        match self.kv.get(fence_kv_key(provider, worker_id)).await? {
            None => Ok(None),
            Some(value) => Ok(Some(serde_json::from_slice(&value)?)),
        }
    }

    async fn set(&self, fence: &WorkerFence) -> anyhow::Result<()> {
        let key = fence_kv_key(fence.provider, &fence.worker_id);
        if fence.state == WorkerPlacementState::Active {
            self.kv.delete(key).await?;
        } else {
            self.kv.put(key, serde_json::to_vec(fence)?.into()).await?;
        }
        Ok(())
    }

    async fn list(&self) -> anyhow::Result<Vec<WorkerFence>> {
        let mut keys = self.kv.keys().await?;
        let mut fences = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key?;
            if let Some(value) = self.kv.get(&key).await? {
                match serde_json::from_slice(&value) {
                    Ok(fence) => fences.push(fence),
                    Err(error) => warn!(%key, %error, "ignoring unreadable worker fence"),
                }
            }
        }
        Ok(fences)
    }
}
