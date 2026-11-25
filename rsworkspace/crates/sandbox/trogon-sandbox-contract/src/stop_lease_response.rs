use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// The outcome of a stop request, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum StopLeaseResponse {
    Stopped,
    Denied(LeaseError),
}
