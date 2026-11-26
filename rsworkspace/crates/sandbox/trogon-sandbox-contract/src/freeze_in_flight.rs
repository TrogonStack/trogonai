use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::FreezeAction;

/// A pause or resume the broker has started but not yet recorded.
///
/// `started_at` and `next_settle_at` use serde's `SystemTime` encoding, an object of
/// `secs_since_epoch` and `nanos_since_epoch`. `settle_attempts` is omitted from JSON when zero and
/// defaults to zero when an older peer leaves it out.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct FreezeInFlight {
    /// Serialized as `thaw`, `true` for a resume and `false` for a pause.
    #[serde(rename = "thaw")]
    pub action: FreezeAction,
    pub started_at: SystemTime,
    /// Times the reconciler asked the worker to settle and was refused.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub settle_attempts: u32,
    /// The reconciler leaves the marker alone until then, so a worker that keeps refusing is not
    /// asked again on every tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_settle_at: Option<SystemTime>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}
