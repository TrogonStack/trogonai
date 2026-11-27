use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{Digest, LiveSnapshotEntry, TenantId};

use super::live_snapshot_store::LiveSnapshotStore;
use super::snapshot_live_error::SnapshotLiveError;
use super::stored_live::{StoredLive, replace};

#[derive(Default)]
pub struct InMemoryLiveSnapshotStore {
    entries: RwLock<HashMap<(TenantId, Digest), StoredLive>>,
}

impl InMemoryLiveSnapshotStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    async fn write(&self, digest: &Digest, next: StoredLive) -> Result<(), SnapshotLiveError> {
        let key = (next.entry.tenant.clone(), digest.clone());
        let mut entries = self.entries.write().await;
        if let Some(current) = entries.get(&key)
            && !replace(current, &next)?
        {
            return Ok(());
        }
        entries.insert(key, next);
        Ok(())
    }
}

#[async_trait]
impl LiveSnapshotStore for InMemoryLiveSnapshotStore {
    async fn put(&self, digest: &Digest, entry: &LiveSnapshotEntry) -> Result<(), SnapshotLiveError> {
        self.put_at(digest, entry, 0).await
    }
    async fn put_at(&self, digest: &Digest, entry: &LiveSnapshotEntry, revision: u64) -> Result<(), SnapshotLiveError> {
        self.write(
            digest,
            StoredLive {
                entry: entry.clone(),
                revision,
                deleted: false,
            },
        )
        .await
    }
    async fn get_owned(
        &self,
        tenant: &TenantId,
        digest: &Digest,
    ) -> Result<Option<LiveSnapshotEntry>, SnapshotLiveError> {
        Ok(self
            .entries
            .read()
            .await
            .get(&(tenant.clone(), digest.clone()))
            .filter(|row| !row.deleted)
            .map(|row| row.entry.clone()))
    }
    async fn get(&self, digest: &Digest) -> Result<Option<LiveSnapshotEntry>, SnapshotLiveError> {
        let entries = self.entries.read().await;
        let mut found = entries
            .iter()
            .filter(|((_, key), row)| key == digest && !row.deleted)
            .map(|(_, row)| row.entry.clone());
        let first = found.next();
        if found.next().is_some() {
            return Err(SnapshotLiveError::AmbiguousDigest);
        }
        Ok(first)
    }
    async fn remove(&self, digest: &Digest) -> Result<(), SnapshotLiveError> {
        let Some(entry) = self.get(digest).await? else {
            return Ok(());
        };
        let revision = self
            .entries
            .read()
            .await
            .get(&(entry.tenant.clone(), digest.clone()))
            .map_or(0, |row| row.revision)
            .checked_add(1)
            .ok_or(SnapshotLiveError::RevisionOverflow)?;
        self.remove_at(digest, &entry, revision).await
    }
    async fn remove_at(
        &self,
        digest: &Digest,
        entry: &LiveSnapshotEntry,
        revision: u64,
    ) -> Result<(), SnapshotLiveError> {
        self.write(
            digest,
            StoredLive {
                entry: entry.clone(),
                revision,
                deleted: true,
            },
        )
        .await
    }
}
