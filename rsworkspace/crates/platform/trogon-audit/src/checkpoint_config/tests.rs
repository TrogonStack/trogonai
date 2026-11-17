use std::time::Duration;

use trogon_std::ZeroDurationError;

use super::*;

#[test]
fn defaults_to_every_thousand_records_or_every_minute() {
    let config = CheckpointConfig::default();
    assert_eq!(config.every_records.get(), 1_000);
    assert_eq!(Duration::from(config.interval), Duration::from_secs(60));
    assert_eq!(Duration::from(config.sign_timeout), Duration::from_secs(2));
    assert_eq!(Duration::from(config.publish_timeout), Duration::from_secs(6));
}

#[test]
fn a_zero_interval_or_operation_timeout_cannot_be_constructed() {
    assert_eq!(NonZeroDuration::try_from(Duration::ZERO), Err(ZeroDurationError));
}

#[test]
fn configured_positive_durations_keep_their_full_precision() {
    let config = CheckpointConfig {
        every_records: NonZeroU64::MIN,
        interval: Duration::from_nanos(1).try_into().unwrap(),
        sign_timeout: Duration::from_nanos(2).try_into().unwrap(),
        publish_timeout: Duration::from_nanos(3).try_into().unwrap(),
    };

    assert_eq!(Duration::from(config.interval), Duration::from_nanos(1));
    assert_eq!(Duration::from(config.sign_timeout), Duration::from_nanos(2));
    assert_eq!(Duration::from(config.publish_timeout), Duration::from_nanos(3));
}
