use serde::{Deserialize, Serialize};

/// Caps how many live leases a tenant may hold for each value of `tag_key`.
///
/// Leases that do not carry `tag_key` are not counted and not limited, so the
/// tenant-wide active lease cap still bounds them.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PerTagQuota {
    pub tag_key: String,
    pub max_active_leases: u32,
}
