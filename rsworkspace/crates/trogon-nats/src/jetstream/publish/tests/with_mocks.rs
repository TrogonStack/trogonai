use super::*;
use crate::jetstream::MockJetStreamPublisher;

#[tokio::test]
async fn publish_event_returns_published_on_success() {
    let publisher = MockJetStreamPublisher::new();
    let result = publish_event(
        &publisher,
        "test.subject".to_string(),
        async_nats::HeaderMap::new(),
        Bytes::from_static(b"payload"),
        Duration::from_secs(10),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn publish_event_returns_publish_failed_on_error() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let result = publish_event(
        &publisher,
        "test.subject".to_string(),
        async_nats::HeaderMap::new(),
        Bytes::from_static(b"payload"),
        Duration::from_secs(10),
    )
    .await;
    assert!(matches!(result, PublishOutcome::PublishFailed(_)));
}
