use std::time::SystemTime;

use trogon_std::NowV7;
use trogon_std::time::EpochClock;

use crate::chain_id::ChainId;
use crate::checkpoint_id::CheckpointId;
use crate::record_id::RecordId;
use crate::stamp::Stamp;

/// Mints a [`Stamp`] from the workspace clock and UUIDv7 abstractions, so tests
/// fix both.
#[derive(Debug, Clone)]
pub struct Stamper<C, G> {
    clock: C,
    ids: G,
}

impl<C: EpochClock, G: NowV7> Stamper<C, G> {
    pub fn new(clock: C, ids: G) -> Self {
        Self { clock, ids }
    }

    pub fn stamp(&self) -> Stamp {
        let now = self.clock.system_time();
        Stamp {
            id: RecordId::generate(&self.ids),
            occurred_at: now,
            recorded_at: now,
        }
    }

    /// A fresh chain id from the same UUIDv7 source, for a writer opening its
    /// chain.
    pub(crate) fn chain_id(&self) -> ChainId {
        ChainId::generate(&self.ids)
    }

    /// A fresh checkpoint id from the same UUIDv7 source.
    pub(crate) fn checkpoint_id(&self) -> CheckpointId {
        CheckpointId::generate(&self.ids)
    }

    /// The time a checkpoint is issued at, from the same clock as the records.
    pub(crate) fn now(&self) -> SystemTime {
        self.clock.system_time()
    }
}

#[cfg(test)]
mod tests;
