use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// The backoff state of an idle pause that did not take effect.
///
/// `retry_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and
/// `nanos_since_epoch`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct IdlePauseRetry {
    /// Idle pauses in a row that did not take effect.
    pub failures: u32,
    /// The idle evaluator does not try again before this.
    pub retry_at: SystemTime,
}
