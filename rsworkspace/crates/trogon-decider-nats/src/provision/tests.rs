use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_nats::jetstream::stream::RetentionPolicy;
use trogon_nats::test_support::JetStreamTestServer;

use super::{
    EnsureBucketError, EnsureStreamError, KvConfigMismatch, StreamConfigMismatch, ensure_bucket, ensure_stream,
};

fn stream_config(name: &str, subject: &str) -> jetstream::stream::Config {
    jetstream::stream::Config {
        name: name.to_string(),
        subjects: vec![subject.to_string()],
        retention: RetentionPolicy::Limits,
        allow_atomic_publish: false,
        ..Default::default()
    }
}

fn bucket_config(bucket: &str, history: i64) -> kv::Config {
    kv::Config {
        bucket: bucket.to_string(),
        history,
        ..Default::default()
    }
}

#[tokio::test]
async fn ensure_stream_creates_a_fresh_stream() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let stream = ensure_stream(&js, stream_config("FRESH_STREAM", "fresh.stream.>"))
        .await
        .expect("fresh stream should be created");

    assert_eq!(stream.cached_info().config.name, "FRESH_STREAM");
}

#[tokio::test]
async fn ensure_stream_opens_an_existing_matching_stream() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let config = stream_config("EXISTING_STREAM", "existing.stream.>");

    ensure_stream(&js, config.clone())
        .await
        .expect("first ensure_stream call should create the stream");

    let stream = ensure_stream(&js, config)
        .await
        .expect("second ensure_stream call should open the existing stream");

    assert_eq!(stream.cached_info().config.name, "EXISTING_STREAM");
}

#[tokio::test]
async fn ensure_stream_rejects_retention_mismatch() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let mut config = stream_config("RETENTION_STREAM", "retention.stream.>");
    config.retention = RetentionPolicy::Limits;
    ensure_stream(&js, config.clone()).await.expect("create stream");

    config.retention = RetentionPolicy::WorkQueue;
    let error = ensure_stream(&js, config)
        .await
        .expect_err("retention mismatch should be rejected");

    assert!(matches!(
        error,
        EnsureStreamError::ConfigMismatch(StreamConfigMismatch::Retention { stream, .. })
            if stream == "RETENTION_STREAM"
    ));
}

#[tokio::test]
async fn ensure_stream_rejects_allow_atomic_publish_mismatch() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    // NATS 2.10 (the testcontainer server version) does not persist
    // `allow_atomic_publish`, so it always round-trips as `false` regardless
    // of what is requested at creation. Creating with the (honored) default
    // of `false` and then requiring `true` still exercises the mismatch
    // path without depending on the server actually storing `true`.
    let mut config = stream_config("ATOMIC_STREAM", "atomic.stream.>");
    config.allow_atomic_publish = false;
    ensure_stream(&js, config.clone()).await.expect("create stream");

    config.allow_atomic_publish = true;
    let error = ensure_stream(&js, config)
        .await
        .expect_err("allow_atomic_publish mismatch should be rejected");

    assert!(matches!(
        error,
        EnsureStreamError::ConfigMismatch(StreamConfigMismatch::AllowAtomicPublish { stream, expected: true, actual: false })
            if stream == "ATOMIC_STREAM"
    ));
}

#[tokio::test]
async fn ensure_stream_rejects_missing_subject() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let config = stream_config("SUBJECT_STREAM", "subject.stream.one");
    ensure_stream(&js, config).await.expect("create stream");

    let wider = stream_config("SUBJECT_STREAM", "subject.stream.two");
    let error = ensure_stream(&js, wider)
        .await
        .expect_err("missing subject should be rejected");

    assert!(matches!(
        error,
        EnsureStreamError::ConfigMismatch(StreamConfigMismatch::MissingSubject { stream, subject })
            if stream == "SUBJECT_STREAM" && subject == "subject.stream.two"
    ));
}

#[tokio::test]
async fn ensure_stream_is_idempotent_under_concurrent_creation() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let config = stream_config("RACE_STREAM", "race.stream.>");

    let (first, second) = tokio::join!(ensure_stream(&js, config.clone()), ensure_stream(&js, config));

    assert_eq!(
        first.expect("first racer should succeed").cached_info().config.name,
        "RACE_STREAM"
    );
    assert_eq!(
        second.expect("second racer should succeed").cached_info().config.name,
        "RACE_STREAM"
    );
}

#[tokio::test]
async fn ensure_bucket_creates_a_fresh_bucket() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;

    let store = ensure_bucket(&js, bucket_config("fresh_bucket", 5))
        .await
        .expect("fresh bucket should be created");

    assert_eq!(store.stream.cached_info().config.max_messages_per_subject, 5);
}

#[tokio::test]
async fn ensure_bucket_opens_an_existing_matching_bucket() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let config = bucket_config("existing_bucket", 3);

    ensure_bucket(&js, config.clone())
        .await
        .expect("first ensure_bucket call should create the bucket");

    let store = ensure_bucket(&js, config)
        .await
        .expect("second ensure_bucket call should open the existing bucket");

    assert_eq!(store.stream.cached_info().config.max_messages_per_subject, 3);
}

#[tokio::test]
async fn ensure_bucket_rejects_history_mismatch() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let mut config = bucket_config("history_bucket", 2);
    ensure_bucket(&js, config.clone()).await.expect("create bucket");

    config.history = 4;
    let error = ensure_bucket(&js, config)
        .await
        .expect_err("history mismatch should be rejected");

    assert!(matches!(
        error,
        EnsureBucketError::ConfigMismatch(KvConfigMismatch::History { bucket, expected: 4, actual: 2 })
            if bucket == "history_bucket"
    ));
}

#[tokio::test]
async fn ensure_bucket_is_idempotent_under_concurrent_creation() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let config = bucket_config("race_bucket", 1);

    let (first, second) = tokio::join!(ensure_bucket(&js, config.clone()), ensure_bucket(&js, config));

    first.expect("first racer should succeed");
    second.expect("second racer should succeed");
}
