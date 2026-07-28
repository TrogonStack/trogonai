//! Host-runtime snapshot policy for scheduler commands.

use trogon_decider_runtime::{CommandSnapshotPolicy, FrequencySnapshot};

use crate::constants::COMMAND_SNAPSHOT_POLICY;
use crate::{PauseSchedule, RemoveSchedule, ResumeSchedule};

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
