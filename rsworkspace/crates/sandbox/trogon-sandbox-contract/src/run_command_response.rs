use serde::{Deserialize, Serialize};

use crate::{CommandOutput, LeaseError, RunCommandStartedResponse};

/// A worker's answer to a [`RunCommandRequest`](crate::RunCommandRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RunCommandResponse {
    Completed(CommandOutput),
    Started(RunCommandStartedResponse),
    Denied(LeaseError),
}
