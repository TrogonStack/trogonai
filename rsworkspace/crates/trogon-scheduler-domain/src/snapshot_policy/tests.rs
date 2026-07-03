use trogon_decider_runtime::CommandSnapshotPolicy;

use super::*;

#[test]
fn command_snapshot_policy_uses_shared_frequency() {
    assert_eq!(
        <PauseSchedule as CommandSnapshotPolicy>::SNAPSHOT_POLICY
            .frequency()
            .get(),
        32
    );
}
