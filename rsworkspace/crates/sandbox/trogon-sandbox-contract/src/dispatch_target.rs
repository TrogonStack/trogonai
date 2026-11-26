use serde::{Deserialize, Serialize};

use crate::{CommandReadinessTarget, DispatchAttemptId, LeaseEpoch, WorkerId};

/// The worker and attempt a lease is dispatched to.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DispatchTarget {
    pub worker_id: WorkerId,
    pub attempt_id: DispatchAttemptId,
    /// Present only when the worker supports lease epochs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<LeaseEpoch>,
}

impl From<CommandReadinessTarget> for DispatchTarget {
    fn from(target: CommandReadinessTarget) -> Self {
        Self {
            worker_id: target.worker_id,
            attempt_id: target.attempt_id,
            epoch: Some(target.epoch),
        }
    }
}
