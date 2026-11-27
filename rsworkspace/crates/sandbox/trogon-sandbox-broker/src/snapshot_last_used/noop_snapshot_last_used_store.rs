use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::Digest;

use super::snapshot_last_used_error::SnapshotLastUsedError;
use super::snapshot_last_used_store::SnapshotLastUsedStore;

pub struct NoopSnapshotLastUsedStore;

#[async_trait]
impl SnapshotLastUsedStore for NoopSnapshotLastUsedStore {
    async fn touch(&self, _digest: &Digest, _at: SystemTime) -> Result<(), SnapshotLastUsedError> {
        Ok(())
    }

    async fn get(&self, _digest: &Digest) -> Result<Option<SystemTime>, SnapshotLastUsedError> {
        Ok(None)
    }
}
