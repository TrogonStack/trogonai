use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Outcome of a [`DeleteNamedSandboxRequest`](crate::DeleteNamedSandboxRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum DeleteNamedSandboxResponse {
    Deleted,
    NotFound,
    Denied(LeaseError),
}
