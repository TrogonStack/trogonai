use serde::{Deserialize, Serialize};

/// What releasing a lease returns to its tenant's quota.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingRefund {
    pub cpu_seconds: u32,
    pub wall_seconds: u32,
}
