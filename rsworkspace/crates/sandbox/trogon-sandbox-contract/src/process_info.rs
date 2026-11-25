use serde::{Deserialize, Serialize};

use crate::{ExecutionId, ProcessName, ProcessState, ReadinessProbe};

/// A named or probed process of a lease.
///
/// Its `state` is flattened into the object, so the JSON carries `state` and, for an exited process, `exit_code`
/// next to the other fields.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub execution_id: ExecutionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ProcessName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<ReadinessProbe>,
    #[serde(flatten)]
    pub state: ProcessState,
}
