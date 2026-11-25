use serde::{Deserialize, Serialize};

use crate::{LeaseError, OperationId};

/// Outcome of a [`DeleteLeaseRequest`](crate::DeleteLeaseRequest), tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum DeleteLeaseResponse {
    Deleted {
        /// The durable operation that carried the delete, when the broker recorded one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<OperationId>,
    },
    Denied(LeaseError),
}
