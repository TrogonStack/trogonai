use async_trait::async_trait;
use trogon_sandbox_contract::{Digest, LeaseId};

use super::ResolvedDigestError;

#[async_trait]
pub trait ResolvedDigestStore: Send + Sync {
    async fn record(&self, lease_id: &LeaseId, digest: &Digest) -> Result<(), ResolvedDigestError>;
    async fn get(&self, lease_id: &LeaseId) -> Result<Option<Digest>, ResolvedDigestError>;
    /// Purges the resolved-digest record for `lease_id`, so the lease can no
    /// longer be traced back to the snapshot digest it resumed from.
    async fn clear(&self, lease_id: &LeaseId) -> Result<(), ResolvedDigestError>;
}
