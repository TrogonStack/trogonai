use serde::{Deserialize, Serialize};

use crate::{DispatchAttemptId, LeaseEpoch, WorkerId};

/// The dispatch a command readiness probe is bound to.
///
/// Unlike [`DispatchTarget`](crate::DispatchTarget), the epoch is required, so a probe always names one exact
/// assignment. Deserialization rejects unknown fields.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReadinessTarget {
    pub worker_id: WorkerId,
    pub attempt_id: DispatchAttemptId,
    pub epoch: LeaseEpoch,
}
