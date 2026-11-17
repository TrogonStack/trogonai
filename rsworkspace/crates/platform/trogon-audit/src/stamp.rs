use std::time::SystemTime;

use crate::record_id::RecordId;

/// A record's identity and timestamps, minted together so a record cannot carry
/// an id from one moment and a time from another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub(crate) id: RecordId,
    pub(crate) occurred_at: SystemTime,
    pub(crate) recorded_at: SystemTime,
}

impl Stamp {
    /// For a record written after the fact, such as a change a projector sees
    /// later than it happened.
    pub fn occurring_at(mut self, occurred_at: SystemTime) -> Self {
        self.occurred_at = occurred_at;
        self
    }

    pub fn id(&self) -> RecordId {
        self.id
    }
}
