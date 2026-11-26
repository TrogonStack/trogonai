use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{LeaseId, LeaseName, LeaseStatus, ProviderKind, SandboxTags, TenantId};

/// Row projection of a lease for by-name lookup responses, such as
/// [`GetLeaseByNameResponse`](crate::GetLeaseByNameResponse).
///
/// It carries a lease's identity and status without its full spec, since a lookup-by-name caller typically needs only
/// those. `name` and `tags` are omitted when absent or empty. `created_at` and `deadline` use serde's `SystemTime`
/// encoding.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseListRow {
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    pub provider: ProviderKind,
    pub status: LeaseStatus,
    pub created_at: SystemTime,
    pub deadline: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<LeaseName>,
    #[serde(default, skip_serializing_if = "SandboxTags::is_empty")]
    pub tags: SandboxTags,
}
