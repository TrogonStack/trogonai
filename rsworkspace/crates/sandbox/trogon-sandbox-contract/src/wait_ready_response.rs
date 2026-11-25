use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a [`WaitReadyRequest`](crate::WaitReadyRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum WaitReadyResponse {
    Ready,
    /// The timeout elapsed with the process still running and not ready.
    NotReady,
    /// The process exited before it became ready.
    Failed {
        exit_code: i32,
    },
    /// The process was started without a readiness probe.
    NoProbe,
    NotFound,
    Unsupported,
    Denied(LeaseError),
}
