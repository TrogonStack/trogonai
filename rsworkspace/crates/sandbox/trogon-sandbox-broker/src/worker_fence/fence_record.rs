use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{ProviderKind, WorkerId};

use super::WorkerPlacementState;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkerFence {
    pub provider: ProviderKind,
    pub worker_id: WorkerId,
    pub state: WorkerPlacementState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub updated_at: SystemTime,
}
