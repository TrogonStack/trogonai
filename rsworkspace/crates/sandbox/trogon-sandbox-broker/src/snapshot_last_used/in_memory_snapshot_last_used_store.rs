use std::collections::HashMap;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::Digest;

use super::snapshot_last_used_error::SnapshotLastUsedError;
use super::snapshot_last_used_store::SnapshotLastUsedStore;

pub struct InMemorySnapshotLastUsedStore {
    records: RwLock<HashMap<String, SystemTime>>,
}

impl InMemorySnapshotLastUsedStore {
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemorySnapshotLastUsedStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SnapshotLastUsedStore for InMemorySnapshotLastUsedStore {
    async fn touch(&self, digest: &Digest, at: SystemTime) -> Result<(), SnapshotLastUsedError> {
        self.records.write().await.insert(digest.as_str().to_string(), at);
        Ok(())
    }

    async fn get(&self, digest: &Digest) -> Result<Option<SystemTime>, SnapshotLastUsedError> {
        Ok(self.records.read().await.get(digest.as_str()).copied())
    }
}
