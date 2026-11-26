use serde::{Deserialize, Serialize};

use crate::{LeaseError, StopSignal};

/// A worker's answer to a [`StopCommandRequest`](crate::StopCommandRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum StopCommandResponse {
    /// The command is no longer running; `signal` is the one that ended it.
    Stopped {
        signal: StopSignal,
    },
    AlreadyCompleted,
    NotFound,
    Unsupported,
    Denied(LeaseError),
}
