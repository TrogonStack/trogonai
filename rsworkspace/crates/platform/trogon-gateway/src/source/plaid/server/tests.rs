use super::*;
use crate::source::plaid::{PlaidApiBaseUrl, PlaidClientId, PlaidSecret};
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::MockJetStreamContext;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::NonZeroDuration;

fn test_config() -> PlaidConfig {
    PlaidConfig {
        client_id: PlaidClientId::new("client-1").unwrap(),
        secret: PlaidSecret::new("secret-1").unwrap(),
        api_base_url: PlaidApiBaseUrl::new("https://sandbox.plaid.com").unwrap(),
        subject_prefix: NatsToken::new("plaid").unwrap(),
        stream_name: NatsToken::new("PLAID").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    }
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

#[tokio::test]
async fn provision_creates_stream_with_dedup_window_spanning_both_sides_of_the_tolerance() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();

    provision(&js, &test_config()).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "PLAID");
    assert_eq!(streams[0].subjects, vec!["plaid.>"]);
    assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    assert_eq!(streams[0].duplicate_window, Duration::from_secs(600));
}
