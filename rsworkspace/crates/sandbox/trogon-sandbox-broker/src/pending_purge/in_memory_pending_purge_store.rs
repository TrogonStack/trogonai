use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::LeaseId;

use super::{PendingPurge, PendingPurgeError, PendingPurgeStore};

#[derive(Default)]
pub struct InMemoryPendingPurgeStore {
    records: RwLock<HashMap<String, PendingPurge>>,
}

impl InMemoryPendingPurgeStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PendingPurgeStore for InMemoryPendingPurgeStore {
    async fn put(&self, purge: &PendingPurge) -> Result<(), PendingPurgeError> {
        self.records
            .write()
            .await
            .insert(purge.lease_id.as_str().to_owned(), purge.clone());
        Ok(())
    }

    async fn get(&self, lease_id: &LeaseId) -> Result<Option<PendingPurge>, PendingPurgeError> {
        Ok(self.records.read().await.get(lease_id.as_str()).cloned())
    }

    async fn clear(&self, lease_id: &LeaseId) -> Result<(), PendingPurgeError> {
        self.records.write().await.remove(lease_id.as_str());
        Ok(())
    }

    async fn all(&self) -> Result<Vec<PendingPurge>, PendingPurgeError> {
        Ok(self.records.read().await.values().cloned().collect())
    }
}
