use std::num::NonZeroU64;

use trogon_std::NonZeroDuration;

use crate::constants::{
    DEFAULT_CHECKPOINT_EVERY_RECORDS, DEFAULT_CHECKPOINT_INTERVAL, DEFAULT_CHECKPOINT_PUBLISH_TIMEOUT,
    DEFAULT_CHECKPOINT_SIGN_TIMEOUT,
};

/// When a writer's head is checkpointed: after `every_records` more records, or
/// once `interval` passes without a checkpoint, whichever comes first
/// (ADR#0086 Decision 6).
///
/// The interval also bounds an idle chain: with no new record its head is signed
/// again, so a verifier can tell a quiet writer from one whose tail was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointConfig {
    pub every_records: NonZeroU64,
    pub interval: NonZeroDuration,
    pub sign_timeout: NonZeroDuration,
    pub publish_timeout: NonZeroDuration,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            every_records: NonZeroU64::new(DEFAULT_CHECKPOINT_EVERY_RECORDS).unwrap_or(NonZeroU64::MIN),
            interval: NonZeroDuration::from_secs_const(DEFAULT_CHECKPOINT_INTERVAL.as_secs()),
            sign_timeout: DEFAULT_CHECKPOINT_SIGN_TIMEOUT,
            publish_timeout: DEFAULT_CHECKPOINT_PUBLISH_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests;
