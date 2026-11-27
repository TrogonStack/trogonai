use serde::{Deserialize, Serialize};

/// Evidence that a watch dropped events because its buffer filled before the caller polled.
///
/// The poll reply reports the loss only to a caller that is still polling. This record puts it
/// on the lease's evidence timeline as well.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileWatchOverflowEvidence {
    pub watch_id: String,
    /// Highest event sequence number dropped so far. Events up to it are gone.
    pub dropped_through: u64,
}
