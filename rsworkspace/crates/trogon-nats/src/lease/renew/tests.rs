use super::*;
use crate::jetstream::{JetStreamPublisher, MockJetStreamKvStore, MockJetStreamPublisher};

fn publish_target() -> KvPublishTarget {
    KvPublishTarget::new("$KV.bucket.", None::<String>, false)
}

#[tokio::test]
async fn renew_uses_update_when_ttl_is_absent() {
    let store = MockJetStreamKvStore::new();
    let revision = renew_store::<_, MockJetStreamPublisher>(
        &store,
        &publish_target(),
        None,
        "key",
        None,
        Bytes::from_static(b"value"),
        7,
    )
    .await
    .unwrap();

    assert_eq!(revision, 1);
    assert_eq!(
        store.update_calls(),
        vec![("key".to_string(), Bytes::from_static(b"value"), 7)]
    );
}

#[tokio::test]
async fn renew_uses_update_when_publisher_is_missing() {
    let store = MockJetStreamKvStore::new();
    let revision = renew_store::<_, MockJetStreamPublisher>(
        &store,
        &publish_target(),
        None,
        "key",
        Some(Duration::from_secs(10)),
        Bytes::from_static(b"value"),
        7,
    )
    .await
    .unwrap();

    assert_eq!(revision, 1);
    assert_eq!(
        store.update_calls(),
        vec![("key".to_string(), Bytes::from_static(b"value"), 7)]
    );
}

#[tokio::test]
async fn renew_rejects_custom_jetstream_prefix() {
    let store = MockJetStreamKvStore::new();
    let publish_target = KvPublishTarget::new("$KV.bucket.", None::<String>, true);
    let publisher = MockJetStreamPublisher::new();

    let error = renew_store(
        &store,
        &publish_target,
        Some(&publisher),
        "key",
        Some(Duration::from_secs(10)),
        Bytes::from_static(b"value"),
        7,
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), kv::UpdateErrorKind::Other);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn renew_publishes_to_put_prefix_when_present() {
    let store = MockJetStreamKvStore::new();
    let publish_target = KvPublishTarget::new("$KV.bucket.", Some("$KV.override."), false);
    let publisher = MockJetStreamPublisher::new();

    let revision = renew_store(
        &store,
        &publish_target,
        Some(&publisher),
        "key",
        Some(Duration::from_secs(10)),
        Bytes::from_static(b"value"),
        7,
    )
    .await
    .unwrap();

    assert_eq!(revision, 1);
    assert_eq!(publisher.published_subjects(), vec!["$KV.override.key"]);
}

#[tokio::test]
async fn renew_uses_default_prefix_when_put_prefix_is_missing() {
    let store = MockJetStreamKvStore::new();
    let publish_target = publish_target();
    let publisher = MockJetStreamPublisher::new();

    let revision = renew_store(
        &store,
        &publish_target,
        Some(&publisher),
        "key",
        Some(Duration::from_secs(10)),
        Bytes::from_static(b"value"),
        7,
    )
    .await
    .unwrap();

    assert_eq!(revision, 1);
    assert_eq!(publisher.published_subjects(), vec!["$KV.bucket.key"]);
}

#[tokio::test]
async fn renew_sets_expected_revision_and_ttl_seconds_headers() {
    let store = MockJetStreamKvStore::new();
    let publish_target = publish_target();
    let publisher = MockJetStreamPublisher::new();

    renew_store(
        &store,
        &publish_target,
        Some(&publisher),
        "key",
        Some(Duration::from_secs(9)),
        Bytes::from_static(b"value"),
        7,
    )
    .await
    .unwrap();

    let mut messages = publisher.published_messages();
    let message = messages.pop().unwrap();
    assert_eq!(
        message
            .headers
            .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
            .map(|value| value.as_str()),
        Some("7")
    );
    assert_eq!(
        message.headers.get(NATS_MESSAGE_TTL).map(|value| value.as_str()),
        Some("9")
    );
}

#[tokio::test]
async fn renew_propagates_publish_errors() {
    let store = MockJetStreamKvStore::new();
    let publish_target = publish_target();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();

    let error = renew_store(
        &store,
        &publish_target,
        Some(&publisher),
        "key",
        Some(Duration::from_secs(10)),
        Bytes::from_static(b"value"),
        7,
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), kv::UpdateErrorKind::Other);
}

#[tokio::test]
async fn renew_returns_publish_ack_sequence() {
    let store = MockJetStreamKvStore::new();
    let publish_target = publish_target();
    let publisher = MockJetStreamPublisher::new();

    let ack1 = JetStreamPublisher::publish_with_headers(&publisher, "preseed", HeaderMap::default(), Bytes::new())
        .await
        .unwrap()
        .into_future()
        .await
        .unwrap();
    assert_eq!(ack1.sequence, 1);

    let revision = renew_store(
        &store,
        &publish_target,
        Some(&publisher),
        "key",
        Some(Duration::from_secs(10)),
        Bytes::from_static(b"value"),
        7,
    )
    .await
    .unwrap();

    assert_eq!(revision, 2);
}
