use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{Digest, LeaseId};

use super::SnapshotPinStore;

pub struct InMemorySnapshotPinStore {
    pins: RwLock<HashMap<LeaseId, Digest>>,
}

impl InMemorySnapshotPinStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pins: RwLock::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl SnapshotPinStore for InMemorySnapshotPinStore {
    async fn pin(&self, lease_id: &LeaseId, digest: &Digest) -> anyhow::Result<()> {
        self.pins.write().await.insert(lease_id.clone(), digest.clone());
        Ok(())
    }

    async fn expected_digest(&self, lease_id: &LeaseId) -> anyhow::Result<Option<Digest>> {
        Ok(self.pins.read().await.get(lease_id).cloned())
    }

    async fn unpin(&self, lease_id: &LeaseId) -> anyhow::Result<()> {
        self.pins.write().await.remove(lease_id);
        Ok(())
    }
}
