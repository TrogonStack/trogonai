use serde::{Deserialize, Serialize};

/// Live CPU and memory readings for one lease, read from the worker at request time.
///
/// Every field is optional because a worker reports only what its runtime exposes.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ResourceGauges {
    /// Cumulative CPU time the sandbox has used, in nanoseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_nanos_total: Option<u64>,
    /// Memory in use now, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// Highest memory use the runtime has recorded, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_peak_bytes: Option<u64>,
    /// Memory ceiling the runtime enforces, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
}
