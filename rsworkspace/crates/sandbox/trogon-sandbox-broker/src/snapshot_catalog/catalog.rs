use async_trait::async_trait;
use trogon_sandbox_contract::SnapshotKvRecord;

use super::snapshot_catalog_error::SnapshotCatalogError;

/// Read and prune access to the catalog records.
#[async_trait]
pub trait SnapshotCatalog: Send + Sync {
    /// Every record. A record that does not decode is skipped, not fatal.
    async fn all(&self) -> Result<Vec<SnapshotKvRecord>, SnapshotCatalogError>;
    async fn remove(&self, record: &SnapshotKvRecord) -> Result<(), SnapshotCatalogError>;
}
