use serde::{Deserialize, Serialize};

use crate::WorkerCapabilities;

/// How many leases a worker can hold, how many it holds now, and its capability bits.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct WorkerCapacity {
    pub max_concurrent_leases: u32,
    pub current_leases: u32,
    /// Absent from older heartbeats, which means no capability bits.
    #[serde(default)]
    pub capabilities: WorkerCapabilities,
}
