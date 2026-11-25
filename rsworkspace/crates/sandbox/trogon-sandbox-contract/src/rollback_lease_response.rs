use serde::{Deserialize, Serialize};

use crate::{LeaseError, OperationId};

/// `Unsupported` means the lease's provider cannot roll back in place.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RollbackLeaseResponse {
    RolledBack {
        /// The durable operation that carried the rollback, when the broker recorded one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<OperationId>,
    },
    Unsupported,
    Denied(LeaseError),
}
