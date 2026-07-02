use super::*;

#[test]
fn bucket_config_uses_the_named_bucket_and_keeps_only_latest_revision() {
    let config = kill_switch_state_bucket_config();
    assert_eq!(config.bucket, KILL_SWITCH_STATE);
    assert_eq!(config.history, 1);
    assert_eq!(config.max_value_size, MAX_VALUE_SIZE);
}
