use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::LeaseId;

use super::{PendingRelease, PendingReleaseError, PendingReleaseStore};

#[derive(Default)]
pub struct InMemoryPendingReleaseStore {
    records: RwLock<HashMap<String, PendingRelease>>,
}

impl InMemoryPendingReleaseStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PendingReleaseStore for InMemoryPendingReleaseStore {
    async fn create(&self, release: &PendingRelease) -> Result<(PendingRelease, bool), PendingReleaseError> {
        let mut records = self.records.write().await;
        match records.get(release.lease_id.as_str()) {
            Some(existing) => Ok((existing.clone(), false)),
            None => {
                records.insert(release.lease_id.as_str().to_owned(), release.clone());
                Ok((release.clone(), true))
            }
        }
    }

    async fn clear(&self, lease_id: &LeaseId) -> Result<(), PendingReleaseError> {
        self.records.write().await.remove(lease_id.as_str());
        Ok(())
    }

    async fn all(&self) -> Result<Vec<PendingRelease>, PendingReleaseError> {
        Ok(self.records.read().await.values().cloned().collect())
    }
}
