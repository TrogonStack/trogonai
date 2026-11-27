use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{Digest, LeaseId};

use super::{ResolvedDigestError, ResolvedDigestStore};

#[derive(Default)]
pub struct InMemoryResolvedDigestStore {
    records: RwLock<HashMap<LeaseId, Digest>>,
}

impl InMemoryResolvedDigestStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ResolvedDigestStore for InMemoryResolvedDigestStore {
    async fn record(&self, lease_id: &LeaseId, digest: &Digest) -> Result<(), ResolvedDigestError> {
        self.records.write().await.insert(lease_id.clone(), digest.clone());
        Ok(())
    }

    async fn get(&self, lease_id: &LeaseId) -> Result<Option<Digest>, ResolvedDigestError> {
        Ok(self.records.read().await.get(lease_id).cloned())
    }

    async fn clear(&self, lease_id: &LeaseId) -> Result<(), ResolvedDigestError> {
        self.records.write().await.remove(lease_id);
        Ok(())
    }
}
