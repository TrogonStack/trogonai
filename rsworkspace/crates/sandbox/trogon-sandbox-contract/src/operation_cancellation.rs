use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// A request to cancel an operation, with the time it was made.
///
/// `requested_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationCancellation {
    pub requested_at: SystemTime,
}
