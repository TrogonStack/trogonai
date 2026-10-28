use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_nats::jetstream::stream::RetentionPolicy;
use trogon_nats::test_support::JetStreamTestServer;

use super::{
    DuplicateWindow, DuplicateWindowExceedsMaxAgeError, EnsureBucketError, EnsureStreamError,
    InvalidDuplicateWindowError, KvConfigMismatchError, StreamConfigMismatchError, apply_duplicate_window,
    ensure_bucket, ensure_stream,
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
        EnsureStreamError::ConfigMismatch(StreamConfigMismatchError::Retention { stream, .. })
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
        EnsureStreamError::ConfigMismatch(StreamConfigMismatchError::AllowAtomicPublish { stream, expected: true, actual: false })
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
        EnsureStreamError::ConfigMismatch(StreamConfigMismatchError::MissingSubject { stream, subject })
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
        EnsureBucketError::ConfigMismatch(KvConfigMismatchError::History { bucket, expected: 4, actual: 2 })
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

#[test]
fn duplicate_window_rejects_zero() {
    let error = DuplicateWindow::try_new(Duration::ZERO).expect_err("zero should be rejected");

    assert_eq!(error, InvalidDuplicateWindowError::Zero);
}

#[test]
fn duplicate_window_rejects_below_minimum() {
    let value = Duration::from_millis(50);

    let error = DuplicateWindow::try_new(value).expect_err("below-minimum window should be rejected");

    assert_eq!(
        error,
        InvalidDuplicateWindowError::BelowMinimum {
            value,
            minimum: DuplicateWindow::MINIMUM,
        }
    );
}

#[test]
fn duplicate_window_accepts_the_minimum() {
    let window = DuplicateWindow::try_new(DuplicateWindow::MINIMUM).expect("minimum window should be accepted");

    assert_eq!(window.as_duration(), DuplicateWindow::MINIMUM);
}

#[test]
fn duplicate_window_accepts_values_above_the_minimum() {
    let value = Duration::from_secs(120);

    let window = DuplicateWindow::try_new(value).expect("window above the minimum should be accepted");

    assert_eq!(window.as_duration(), value);
}

#[test]
fn duplicate_window_round_trips_through_duration_conversions() {
    let value = Duration::from_secs(90);

    let window = DuplicateWindow::try_from(value).expect("a valid window converts from a duration");

    assert_eq!(Duration::from(window), value);
    assert_eq!(
        DuplicateWindow::try_from(Duration::ZERO).expect_err("zero should be rejected"),
        InvalidDuplicateWindowError::Zero
    );
}

#[test]
fn apply_duplicate_window_sets_the_field_when_max_age_is_unset() {
    let config = stream_config("DUP_STREAM", "dup.stream.>");
    let window = DuplicateWindow::try_new(Duration::from_secs(30)).expect("valid window");

    let config = apply_duplicate_window(config, window).expect("an unset max_age accepts any window");

    assert_eq!(config.duplicate_window, Duration::from_secs(30));
}

#[test]
fn apply_duplicate_window_accepts_a_window_at_or_below_max_age() {
    let mut config = stream_config("DUP_STREAM", "dup.stream.>");
    config.max_age = Duration::from_secs(60);
    let window = DuplicateWindow::try_new(Duration::from_secs(60)).expect("valid window");

    let config = apply_duplicate_window(config, window).expect("a window equal to max_age should be accepted");

    assert_eq!(config.duplicate_window, Duration::from_secs(60));
}

#[test]
fn apply_duplicate_window_rejects_a_window_wider_than_max_age() {
    let mut config = stream_config("DUP_STREAM", "dup.stream.>");
    config.max_age = Duration::from_secs(30);
    let window = DuplicateWindow::try_new(Duration::from_secs(60)).expect("valid window");

    let error = apply_duplicate_window(config, window).expect_err("a window wider than max_age should be rejected");

    assert_eq!(
        error,
        DuplicateWindowExceedsMaxAgeError {
            duplicate_window: Duration::from_secs(60),
            max_age: Duration::from_secs(30),
        }
    );
}

#[tokio::test]
async fn ensure_stream_provisions_a_stream_carrying_the_configured_duplicate_window() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let window = DuplicateWindow::try_new(Duration::from_secs(45)).expect("valid window");
    let config = apply_duplicate_window(stream_config("DUP_WINDOW_STREAM", "dup.window.stream.>"), window)
        .expect("an unset max_age accepts any window");

    let stream = ensure_stream(&js, config)
        .await
        .expect("stream with a configured duplicate window should be created");

    assert_eq!(stream.cached_info().config.duplicate_window, Duration::from_secs(45));
}
