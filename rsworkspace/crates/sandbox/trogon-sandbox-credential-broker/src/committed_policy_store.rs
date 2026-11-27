use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, TenantId};

use crate::{CommittedPolicy, PolicyStoreError};

/// Read side of the committed-policy record, keyed by tenant and lease.
#[async_trait]
pub trait CommittedPolicyStore: Send + Sync {
    async fn committed(&self, tenant: &TenantId, lease: &LeaseId) -> Result<Option<CommittedPolicy>, PolicyStoreError>;
}
