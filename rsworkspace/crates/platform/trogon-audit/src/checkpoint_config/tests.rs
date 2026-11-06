use super::*;

#[test]
fn defaults_to_every_thousand_records_or_every_minute() {
    let config = CheckpointConfig::default();
    assert_eq!(config.every_records.get(), 1_000);
    assert_eq!(config.interval, Duration::from_secs(60));
}
