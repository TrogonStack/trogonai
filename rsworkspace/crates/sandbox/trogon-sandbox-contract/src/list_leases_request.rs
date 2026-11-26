use serde::{Deserialize, Serialize};

use crate::{LeaseStatus, SandboxTags, TenantId};

/// Client-facing request to list leases owned by `tenant`, optionally filtered by `status_filter` and `tags` and
/// paginated by an opaque `cursor`.
///
/// `limit` is the page size. The broker uses 50 when it is absent and caps it at 200. `tags` keeps only leases whose
/// tags contain every entry here, by exact match on key and value. Empty means no tag filter, and it is omitted when
/// empty. A filter of more than five entries is rejected, since no lease can carry that many tags.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListLeasesRequest {
    pub tenant: TenantId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_filter: Option<LeaseStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "SandboxTags::is_empty")]
    pub tags: SandboxTags,
}
