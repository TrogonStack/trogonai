use serde::{Deserialize, Serialize};

use crate::{CommandLaunchOrigin, ExecutionId};

/// A worker's reply to a detached [`RunCommandRequest`](crate::RunCommandRequest) once it accepts the command.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunCommandStartedResponse {
    pub execution_id: ExecutionId,
    #[serde(default, skip_serializing_if = "CommandLaunchOrigin::is_unknown")]
    pub origin: CommandLaunchOrigin,
}
