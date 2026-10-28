use super::*;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{MockJetStreamContext, StreamMaxAge};
use trogon_std::NonZeroDuration;

use crate::source::google_pubsub::config::{GooglePubSubTransportConfig, StreamingPullConfig};
use crate::source::google_pubsub::credentials::fixtures::fake_credentials;
use crate::source::google_pubsub::pub_sub_subscription_name::PubSubSubscriptionName;

fn test_config() -> GooglePubSubConfig {
    GooglePubSubConfig {
        subject_prefix: NatsToken::new("google-pubsub-primary").unwrap(),
        stream_name: NatsToken::new("GOOGLE_PUBSUB_PRIMARY").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        transport: GooglePubSubTransportConfig::StreamingPull(StreamingPullConfig {
            subscription: PubSubSubscriptionName::new("projects/p/subscriptions/sub").unwrap(),
            max_outstanding_messages: None,
            credentials: fake_credentials(),
        }),
    }
}

#[tokio::test]
async fn provision_creates_stream() {
    let js = MockJetStreamContext::new();

    provision(&js, &test_config()).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "GOOGLE_PUBSUB_PRIMARY");
    assert_eq!(streams[0].subjects, vec!["google-pubsub-primary.>"]);
    assert_eq!(streams[0].max_age, Duration::from_secs(3600));
}

#[tokio::test]
async fn provision_propagates_error() {
    let js = MockJetStreamContext::new();
    js.fail_next();

    let result = provision(&js, &test_config()).await;

    assert!(result.is_err());
}
