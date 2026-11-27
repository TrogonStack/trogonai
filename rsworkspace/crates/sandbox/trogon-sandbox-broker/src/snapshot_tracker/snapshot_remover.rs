use async_trait::async_trait;
use trogon_sandbox_contract::{Digest, TenantId};

use super::snapshot_removal::SnapshotRemoval;
use super::snapshot_removal_error::SnapshotRemovalError;

/// Deletes a tenant's snapshot artifact. `NotFound` covers both an unknown
/// digest and one owned by another tenant, so ownership is never revealed.
#[async_trait]
pub trait SnapshotRemover: Send + Sync {
    async fn remove(&self, tenant: &TenantId, digest: &Digest) -> Result<SnapshotRemoval, SnapshotRemovalError>;
}
