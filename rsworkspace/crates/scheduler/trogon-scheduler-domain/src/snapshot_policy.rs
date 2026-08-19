//! Host-runtime snapshot policy for scheduler commands.
//!
//! Each impl forwards to the command's own
//! [`Decider::SNAPSHOT_CADENCE`](trogon_decider::Decider::SNAPSHOT_CADENCE) rather than naming a
//! frequency of its own. That declaration also travels to the WASM host in the module descriptor,
//! so the native and sandboxed paths snapshot the same command at the same rate by construction.

use trogon_decider::{Decider, SnapshotCadence};
use trogon_decider_runtime::CommandSnapshotPolicy;

use crate::{PauseSchedule, RemoveSchedule, ResumeSchedule};

impl CommandSnapshotPolicy for PauseSchedule {
    type SnapshotPolicy = SnapshotCadence;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = Self::SNAPSHOT_CADENCE;
}

impl CommandSnapshotPolicy for RemoveSchedule {
    type SnapshotPolicy = SnapshotCadence;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = Self::SNAPSHOT_CADENCE;
}

impl CommandSnapshotPolicy for ResumeSchedule {
    type SnapshotPolicy = SnapshotCadence;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = Self::SNAPSHOT_CADENCE;
}

#[cfg(test)]
mod tests;
