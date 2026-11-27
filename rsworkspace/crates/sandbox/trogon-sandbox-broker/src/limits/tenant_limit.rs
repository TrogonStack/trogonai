use serde::{Deserialize, Serialize};

use super::PerTagQuota;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantLimit {
    pub max_cpu_seconds_per_request: u32,
    pub max_memory_mib_per_request: u32,
    /// Optional cap on live leases per value of one tag key, such as one
    /// cap per `user`. Absent means no per-tag limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_tag_quota: Option<PerTagQuota>,
}
