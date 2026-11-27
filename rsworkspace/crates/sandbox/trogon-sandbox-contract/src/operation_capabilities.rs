use serde::{Deserialize, Serialize};

use crate::CreateCancellationSupport;

/// The operation features the broker offers.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationCapabilities {
    pub supports_create_cancellation: CreateCancellationSupport,
}
