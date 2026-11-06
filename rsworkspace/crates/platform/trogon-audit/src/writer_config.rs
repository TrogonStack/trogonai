use std::time::Duration;

use crate::constants::{
    DEFAULT_BEST_EFFORT_CAPACITY, DEFAULT_DRAIN_INTERVAL, DEFAULT_RETRY_BUFFER_CAPACITY, STREAM_DUPLICATE_WINDOW,
};

/// Bounds for the writer's two in-memory buffers. A capacity of zero is read as
/// one, because a buffer that holds nothing cannot be told from a broken one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterConfig {
    /// Best-effort records waiting for an asynchronous publish.
    pub best_effort_capacity: usize,
    /// Refusal records waiting to be retried. Full means audit-degraded.
    pub retry_capacity: usize,
    /// How often the drain task retries the chain's head and the buffers when
    /// nothing wakes it.
    pub drain_interval: Duration,
    /// The stream's duplicate window. A retry made later than this after a
    /// record's first publish is counted as one that may store it twice.
    pub duplicate_window: Duration,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            best_effort_capacity: DEFAULT_BEST_EFFORT_CAPACITY,
            retry_capacity: DEFAULT_RETRY_BUFFER_CAPACITY,
            drain_interval: DEFAULT_DRAIN_INTERVAL,
            duplicate_window: STREAM_DUPLICATE_WINDOW,
        }
    }
}
