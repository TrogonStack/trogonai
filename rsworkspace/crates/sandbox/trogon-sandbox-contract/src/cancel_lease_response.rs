use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// Reply to a lease cancel. Cancel began as a plain publish, so the broker only answers a
/// cancel sent with a reply subject; publishers that do not ask keep working unchanged.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CancelLeaseResponse {
    Cancelled,
    Denied(LeaseError),
}
