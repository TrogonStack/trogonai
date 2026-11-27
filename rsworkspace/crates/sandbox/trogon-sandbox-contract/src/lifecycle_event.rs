use serde::{Deserialize, Serialize};

use crate::{OutputStream, SandboxId, SandboxPhase};

/// A lease lifecycle or phase progress event, tagged by `status`.
///
/// `detail` is omitted from JSON when absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum LifecycleEvent {
    Started {
        sandbox_id: SandboxId,
    },
    Completed {
        sandbox_id: SandboxId,
    },
    Failed {
        sandbox_id: SandboxId,
        error: String,
    },
    Cancelled {
        sandbox_id: SandboxId,
    },
    PhaseStarted {
        phase: SandboxPhase,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    PhaseCompleted {
        phase: SandboxPhase,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    PhaseFailed {
        phase: SandboxPhase,
        duration_ms: u64,
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Output {
        stream: OutputStream,
        text: String,
    },
}
