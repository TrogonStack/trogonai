use super::*;

#[test]
fn command_snapshot_policy_uses_expected_frequency() {
    assert_eq!(COMMAND_SNAPSHOT_POLICY.frequency().get(), 32);
    assert_eq!(COMMAND_SNAPSHOT_POLICY.frequency(), COMMAND_SNAPSHOT_EVERY);
}
