use super::*;

#[test]
fn zoom_config_clone_roundtrips() {
    let config = ZoomConfig {
        secret_token: ZoomSecretToken::new("test-token").unwrap(),
        subject_prefix: NatsToken::new("zoom").unwrap(),
        stream_name: NatsToken::new("ZOOM").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    };

    let clone = config.clone();
    assert_eq!(clone.subject_prefix.as_str(), "zoom");
    assert_eq!(clone.stream_name.as_str(), "ZOOM");
}
