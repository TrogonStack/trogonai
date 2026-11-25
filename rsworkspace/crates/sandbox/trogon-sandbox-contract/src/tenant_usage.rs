use serde::{Deserialize, Serialize};

/// Counters summed over a tenant's live leases and the released leases whose final figures the
/// broker still keeps.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TenantUsage {
    pub live_leases: u64,
    /// Released leases folded into the totals.
    #[serde(default)]
    pub ended_leases: u64,
    pub commands_run: u64,
    pub command_output_bytes: u64,
    pub file_bytes_in: u64,
    pub file_bytes_out: u64,
    pub running_seconds: u64,
    pub paused_seconds: u64,
    #[serde(default)]
    pub suspended_seconds: u64,
}
