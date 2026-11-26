use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Outcome of an [`ExtendLeaseTimeoutRequest`](crate::ExtendLeaseTimeoutRequest), tagged by `outcome`.
///
/// `new_deadline` uses serde's `SystemTime` encoding.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ExtendLeaseTimeoutResponse {
    Extended { new_deadline: SystemTime },
    Denied(LeaseError),
}
