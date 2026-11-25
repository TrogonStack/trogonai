use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{LeaseError, OperationId, SuspendLevel};

/// `Suspended.level` is the level the suspend landed on. `reap_at` is when
/// the broker releases the lease if nobody wakes it. A provider that cannot
/// suspend to the requested level is refused as
/// [`LeaseError::SuspendUnsupported`] inside `Denied`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SuspendLeaseResponse {
    Suspended {
        level: SuspendLevel,
        reap_at: SystemTime,
        /// The durable operation that carried the suspend, when the broker recorded one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<OperationId>,
    },
    Denied(LeaseError),
}
