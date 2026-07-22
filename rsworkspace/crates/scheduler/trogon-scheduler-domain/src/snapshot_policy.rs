//! Host-runtime snapshot policy for scheduler commands.

use std::num::NonZeroU64;

use trogon_decider_runtime::{CommandSnapshotPolicy, FrequencySnapshot};

use crate::{PauseSchedule, RemoveSchedule, ResumeSchedule};

const COMMAND_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).unwrap();

pub const COMMAND_SNAPSHOT_POLICY: FrequencySnapshot = FrequencySnapshot::new(COMMAND_SNAPSHOT_EVERY);

impl CommandSnapshotPolicy for PauseSchedule {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = COMMAND_SNAPSHOT_POLICY;
}

impl CommandSnapshotPolicy for RemoveSchedule {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = COMMAND_SNAPSHOT_POLICY;
}

impl CommandSnapshotPolicy for ResumeSchedule {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests;
