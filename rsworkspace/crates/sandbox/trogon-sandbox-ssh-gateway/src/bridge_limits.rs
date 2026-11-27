pub use crate::constants::{
    DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_BUFFERED_BYTES, DEFAULT_MAX_CHANNELS, DEFAULT_MAX_SESSIONS,
};
use std::num::NonZeroUsize;
use std::time::Duration;

/// The bounds a worker enforces on the sessions it bridges.
#[derive(Debug, Clone, Copy)]
pub struct BridgeLimits {
    pub max_sessions: NonZeroUsize,
    pub idle_timeout: Duration,
    pub max_buffered_bytes: usize,
    pub max_channels: NonZeroUsize,
}

impl Default for BridgeLimits {
    fn default() -> Self {
        Self {
            max_sessions: DEFAULT_MAX_SESSIONS,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            max_buffered_bytes: DEFAULT_MAX_BUFFERED_BYTES,
            max_channels: DEFAULT_MAX_CHANNELS,
        }
    }
}
