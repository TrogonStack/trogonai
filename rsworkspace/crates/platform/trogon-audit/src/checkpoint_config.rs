use std::num::NonZeroU64;
use std::time::Duration;

use crate::constants::{DEFAULT_CHECKPOINT_EVERY_RECORDS, DEFAULT_CHECKPOINT_INTERVAL};

/// When a writer's head is checkpointed: after `every_records` more records, or
/// once `interval` passes without a checkpoint, whichever comes first
/// (ADR#0086 Decision 6).
///
/// The interval also bounds an idle chain: with no new record its head is signed
/// again, so a verifier can tell a quiet writer from one whose tail was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointConfig {
    pub every_records: NonZeroU64,
    pub interval: Duration,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            every_records: NonZeroU64::new(DEFAULT_CHECKPOINT_EVERY_RECORDS).unwrap_or(NonZeroU64::MIN),
            interval: DEFAULT_CHECKPOINT_INTERVAL,
        }
    }
}

#[cfg(test)]
mod tests;
