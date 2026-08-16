use trogon_decider_runtime::CommandSnapshotPolicy;

use super::*;
use crate::constants::COMMAND_SNAPSHOT_CADENCE;

#[test]
fn the_host_policy_forwards_the_cadence_the_command_declares() {
    assert_eq!(
        <PauseSchedule as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        COMMAND_SNAPSHOT_CADENCE
    );
    assert_eq!(
        <RemoveSchedule as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
        <ResumeSchedule as CommandSnapshotPolicy>::SNAPSHOT_POLICY
    );
    assert_eq!(
        COMMAND_SNAPSHOT_CADENCE
            .frequency()
            .expect("a declared frequency")
            .get(),
        32
    );
}
