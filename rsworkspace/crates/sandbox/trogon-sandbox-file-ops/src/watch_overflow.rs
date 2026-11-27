/// A watch dropped events because its buffer filled before it was polled.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WatchOverflow {
    pub watch_id: String,
    /// Highest event sequence number dropped so far.
    pub dropped_through: u64,
}
