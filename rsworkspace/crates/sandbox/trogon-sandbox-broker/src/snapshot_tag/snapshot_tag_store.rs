use async_trait::async_trait;
use trogon_sandbox_contract::{SnapshotTag, SnapshotTagRecord, TenantId};

use super::snapshot_tag_store_error::SnapshotTagStoreError;

#[async_trait]
pub trait SnapshotTagStore: Send + Sync {
    async fn get(&self, tag: &SnapshotTag) -> Result<Option<SnapshotTagRecord>, SnapshotTagStoreError>;
    async fn get_entry(&self, tag: &SnapshotTag) -> Result<Option<(SnapshotTagRecord, u64)>, SnapshotTagStoreError>;
    /// `Some(0)` creates only when absent; positive revisions compare the current
    /// record. `None` permits an unconditional update.
    async fn set(
        &self,
        tag: &SnapshotTag,
        record: SnapshotTagRecord,
        expected_revision: Option<u64>,
    ) -> Result<u64, SnapshotTagStoreError>;
    async fn clear(&self, tag: &SnapshotTag, expected_revision: u64) -> Result<(), SnapshotTagStoreError>;
    async fn list(&self, tenant: &TenantId) -> Result<Vec<(SnapshotTag, SnapshotTagRecord)>, SnapshotTagStoreError>;
    /// Every tenant that owns at least one tag, derived from the tenant-scoped
    /// keys so a sweeper needs no configured tenant list. Sorted, deduplicated.
    /// A store that cannot enumerate returns none, which leaves a sweeper on
    /// its configured list.
    async fn tenants(&self) -> Result<Vec<TenantId>, SnapshotTagStoreError> {
        Ok(Vec::new())
    }
}
