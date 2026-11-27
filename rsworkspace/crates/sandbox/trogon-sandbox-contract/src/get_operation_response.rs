use serde::{Deserialize, Serialize};

use crate::{LeaseError, OperationRecord};

/// The reply to an operation lookup, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum GetOperationResponse {
    Found { operation: Box<OperationRecord> },
    NotFound,
    Denied(LeaseError),
}
