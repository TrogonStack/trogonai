use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Who is driving a running operation.
///
/// A write carrying an older generation is fenced out, so a driver that lost its claim cannot overwrite the new
/// owner. `renewed_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and
/// `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationClaim {
    pub owner: String,
    pub generation: u64,
    pub renewed_at: SystemTime,
}
