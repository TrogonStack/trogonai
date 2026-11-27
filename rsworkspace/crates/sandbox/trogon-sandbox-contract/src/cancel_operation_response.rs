use serde::{Deserialize, Serialize};

use crate::{LeaseError, OperationState};

/// The reply to an operation cancellation, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CancelOperationResponse {
    Canceled,
    Pending,
    AlreadyCompleted { state: OperationState },
    Unsupported,
    Denied(LeaseError),
}
