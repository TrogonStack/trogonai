use serde::{Deserialize, Serialize};

use crate::{DispatchAttemptId, LeaseEpoch, SandboxId, WorkerId};

/// The lease a worker grants in reply to a dispatch.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderLease {
    pub sandbox_id: SandboxId,
    pub evidence_subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<WorkerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<DispatchAttemptId>,
    /// An echo of the epoch the worker adopted. Absent from workers that do not support epochs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<LeaseEpoch>,
}
