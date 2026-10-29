use super::*;

#[test]
fn credential_snapshot_policy_uses_expected_frequency() {
    assert_eq!(CREDENTIAL_SNAPSHOT_POLICY.frequency().get(), 32);
    assert_eq!(CREDENTIAL_SNAPSHOT_POLICY.frequency(), CREDENTIAL_SNAPSHOT_EVERY);
}
