use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// A dispatch whose reply has not been committed yet.
///
/// `sent_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and
/// `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingDispatch {
    /// When the dispatch was first sent; a re-send waits out the dispatch timeout from here so it
    /// never races the original request.
    pub sent_at: SystemTime,
    /// The provider dispatch payload as sent, kept as opaque JSON.
    pub payload: serde_json::Value,
}
