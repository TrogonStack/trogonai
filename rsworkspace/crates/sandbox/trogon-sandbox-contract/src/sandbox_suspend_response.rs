use serde::{Deserialize, Serialize};

use crate::{DormancyArtifact, LeaseError};

/// A worker's answer to a suspend request, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SandboxSuspendResponse {
    Done {
        /// Where the state went, when the worker keeps it apart from the sandbox.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<DormancyArtifact>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact_bytes: Option<u64>,
    },
    /// The worker cannot suspend to the requested level.
    Unsupported,
    Denied(LeaseError),
}
