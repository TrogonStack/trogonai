use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Outcome of a request to replace a running lease's egress network policy, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum NetworkPolicyUpdateResponse {
    Applied,
    Denied(LeaseError),
}
