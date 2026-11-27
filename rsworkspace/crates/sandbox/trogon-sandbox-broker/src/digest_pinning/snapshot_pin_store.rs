use async_trait::async_trait;
use trogon_sandbox_contract::{Digest, LeaseId};

#[async_trait]
pub trait SnapshotPinStore: Send + Sync {
    async fn pin(&self, lease_id: &LeaseId, digest: &Digest) -> anyhow::Result<()>;
    async fn expected_digest(&self, lease_id: &LeaseId) -> anyhow::Result<Option<Digest>>;
    async fn unpin(&self, lease_id: &LeaseId) -> anyhow::Result<()>;
}
