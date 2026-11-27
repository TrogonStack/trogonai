use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::SnapshotKvRecord;

use super::catalog::SnapshotCatalog;
use super::snapshot_catalog_error::SnapshotCatalogError;

#[derive(Default)]
pub struct InMemorySnapshotCatalog {
    records: Mutex<Vec<SnapshotKvRecord>>,
}

impl InMemorySnapshotCatalog {
    pub async fn insert(&self, record: SnapshotKvRecord) {
        self.records.lock().await.push(record);
    }
}

#[async_trait]
impl SnapshotCatalog for InMemorySnapshotCatalog {
    async fn all(&self) -> Result<Vec<SnapshotKvRecord>, SnapshotCatalogError> {
        Ok(self.records.lock().await.clone())
    }

    async fn remove(&self, record: &SnapshotKvRecord) -> Result<(), SnapshotCatalogError> {
        self.records.lock().await.retain(|r| r != record);
        Ok(())
    }
}
