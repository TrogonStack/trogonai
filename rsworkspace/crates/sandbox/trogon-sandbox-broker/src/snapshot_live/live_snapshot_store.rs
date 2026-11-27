use async_trait::async_trait;
use trogon_sandbox_contract::{Digest, LiveSnapshotEntry, TenantId};

use super::snapshot_live_error::SnapshotLiveError;

#[async_trait]
pub trait LiveSnapshotStore: Send + Sync {
    async fn put(&self, digest: &Digest, entry: &LiveSnapshotEntry) -> Result<(), SnapshotLiveError>;
    async fn get(&self, digest: &Digest) -> Result<Option<LiveSnapshotEntry>, SnapshotLiveError>;
    async fn remove(&self, digest: &Digest) -> Result<(), SnapshotLiveError>;
    async fn get_owned(
        &self,
        tenant: &TenantId,
        digest: &Digest,
    ) -> Result<Option<LiveSnapshotEntry>, SnapshotLiveError> {
        Ok(self.get(digest).await?.filter(|entry| entry.tenant == *tenant))
    }
    async fn put_at(
        &self,
        digest: &Digest,
        entry: &LiveSnapshotEntry,
        _revision: u64,
    ) -> Result<(), SnapshotLiveError> {
        self.put(digest, entry).await
    }
    async fn remove_at(
        &self,
        _digest: &Digest,
        _entry: &LiveSnapshotEntry,
        _revision: u64,
    ) -> Result<(), SnapshotLiveError> {
        Err(SnapshotLiveError::RemovalUnavailable)
    }
}
