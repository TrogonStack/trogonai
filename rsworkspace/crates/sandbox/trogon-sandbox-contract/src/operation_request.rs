use serde::{Deserialize, Serialize};

use crate::{
    CreateQuotaStep, DispatchAttemptId, LeaseRequest, OciArtifactRef, ProviderDispatchPayload, SuspendRequestLevel,
};

/// The request an operation re-drives from after a restart, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OperationRequest {
    Pause {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_paused_seconds: Option<u32>,
    },
    Resume,
    Suspend {
        #[serde(default)]
        level: SuspendRequestLevel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_suspended_seconds: Option<u32>,
    },
    Wake,
    Delete,
    /// Resets the target lease to a snapshot, keeping its id.
    Rollback {
        snapshot: OciArtifactRef,
    },
    /// A create, or a restore when the image is a snapshot. The lease id is the operation's target, and the attempt
    /// id is picked with it so a re-drive sends the same attempt.
    Create {
        /// The caller's request, with its operation id cleared.
        request: Box<LeaseRequest>,
        attempt_id: DispatchAttemptId,
        /// What was sent to a targeted worker, stored before the send so a re-drive resends it unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dispatch: Option<Box<ProviderDispatchPayload>>,
        /// Where the tenant quota stands for a create that holds no dispatch attempt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quota: Option<CreateQuotaStep>,
    },
}
