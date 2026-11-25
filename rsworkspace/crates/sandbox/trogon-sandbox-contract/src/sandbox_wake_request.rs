use serde::{Deserialize, Serialize};

use crate::{DormancyArtifact, LeaseId, SuspendLevel};

/// Broker-to-worker request to wake a suspended lease. Routed like a sandbox
/// suspend request; a repeat of the same `generation` is success.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxWakeRequest {
    pub lease_id: LeaseId,
    pub generation: u64,
    pub level: SuspendLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<DormancyArtifact>,
}
