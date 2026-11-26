use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{LeaseId, LeaseStatus, ProviderKind, SandboxTags, TenantId};

/// Tenant-facing summary of a lease, returned by [`ListLeasesResponse`](crate::ListLeasesResponse).
///
/// `last_activity_at` is the last time the broker saw activity on the lease. It is omitted until the first activity on
/// a lease that has an idle timeout. `tags` is omitted when empty. Times use serde's `SystemTime` encoding.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseSummary {
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
    pub status: LeaseStatus,
    pub provider: ProviderKind,
    #[serde(default, skip_serializing_if = "SandboxTags::is_empty")]
    pub tags: SandboxTags,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<SystemTime>,
}
