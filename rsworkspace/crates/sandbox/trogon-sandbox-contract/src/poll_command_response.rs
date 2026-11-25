use serde::{Deserialize, Serialize};

use crate::{CommandOutput, LeaseError};

/// A worker's answer to a [`PollCommandRequest`](crate::PollCommandRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum PollCommandResponse {
    Running,
    Completed(CommandOutput),
    NotFound,
    Denied(LeaseError),
}
