use serde::{Deserialize, Serialize};

use crate::TagLimits;

/// A tenant's broker-side limits next to what it has consumed so far, so a caller can check
/// capacity before asking for a lease.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct AccountLimits {
    /// Live leases now, in any status other than released or failed.
    pub active_leases: u32,
    pub max_active_leases: u32,
    /// CPU seconds reserved this billing month.
    pub cpu_seconds_used: u32,
    pub cpu_seconds_cap: u32,
    /// Wall-clock seconds reserved this billing month.
    pub wall_seconds_used: u32,
    pub wall_seconds_cap: u32,
    /// Largest `cpu_seconds` a single lease request may ask for.
    pub max_cpu_seconds_per_request: u32,
    /// Largest `memory_mib` a single lease request may ask for.
    pub max_memory_mib_per_request: u32,
    /// Present when the tenant has a per-tag live lease cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_tag: Option<TagLimits>,
}
