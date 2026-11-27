use std::collections::HashSet;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, PullSecretId, TenantId};

use crate::PolicyStoreError;

/// Write side of the committed-policy record.
#[async_trait]
pub trait CommittedPolicyWriter: Send + Sync {
    /// Commits `references` for the lease and returns the committed revision.
    ///
    /// The first commit is revision 1 and each commit that changes the set is one higher than the last.
    /// Committing the set that is already committed returns the current revision, so a retried create does not
    /// move it.
    async fn commit(
        &self,
        tenant: &TenantId,
        lease: &LeaseId,
        references: HashSet<PullSecretId>,
    ) -> Result<u64, PolicyStoreError>;

    /// Deletes the lease's record. Deleting a record that is not there is fine.
    async fn revoke(&self, tenant: &TenantId, lease: &LeaseId) -> Result<(), PolicyStoreError>;
}
