use serde::{Deserialize, Serialize};

use crate::{LeaseError, ProcessInfo};

/// A worker's answer to a [`ListProcessesRequest`](crate::ListProcessesRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ListProcessesResponse {
    Processes { processes: Vec<ProcessInfo> },
    Unsupported,
    Denied(LeaseError),
}
