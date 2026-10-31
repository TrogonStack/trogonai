use super::*;
use crate::source::plaid::PlaidEnvironment;

#[test]
fn plaid_config_clone_roundtrips() {
    let config = PlaidConfig {
        client_id: PlaidClientId::new("client-id").unwrap(),
        secret: PlaidSecret::new("test-secret").unwrap(),
        api_base_url: PlaidApiBaseUrl::new(PlaidEnvironment::Sandbox.default_api_base_url()).unwrap(),
        subject_prefix: NatsToken::new("plaid").unwrap(),
        stream_name: NatsToken::new("PLAID").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    };

    let clone = config.clone();
    assert_eq!(clone.subject_prefix.as_str(), "plaid");
    assert_eq!(clone.stream_name.as_str(), "PLAID");
    assert_eq!(clone.client_id.as_str(), "client-id");
}
