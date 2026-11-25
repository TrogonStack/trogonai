use serde::{Deserialize, Serialize};

use crate::{ProcessName, ReadinessProbe};

/// The identity and readiness of a detached command.
///
/// It lets a long-running process, such as a dev server or a worker, be found again without storing its execution id
/// and be waited on until it can serve. A detached [`RunCommandRequest`](crate::RunCommandRequest) carries it in
/// [`CommandExecution::Detached`](crate::CommandExecution::Detached).
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProcessSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ProcessName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<ReadinessProbe>,
}
