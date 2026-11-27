use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::Digest;

use super::snapshot_last_used_error::SnapshotLastUsedError;

#[async_trait]
pub trait SnapshotLastUsedStore: Send + Sync {
    async fn touch(&self, digest: &Digest, at: SystemTime) -> Result<(), SnapshotLastUsedError>;
    async fn get(&self, digest: &Digest) -> Result<Option<SystemTime>, SnapshotLastUsedError>;
}
