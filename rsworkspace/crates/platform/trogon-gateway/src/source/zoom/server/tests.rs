use super::*;
use axum::body::Body;
use axum::http::Request;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{ClaimBucket, ClaimBucketBinding};
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};
use trogon_std::time::FixedEpochClock;

type HmacSha256 = Hmac<Sha256>;

const TEST_NOW: u64 = 1_700_000_000;
fn test_secret() -> String {
    ["test-", "token"].concat()
}

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn test_config() -> ZoomConfig {
    ZoomConfig {
        secret_token: ZoomSecretToken::new(test_secret()).unwrap(),
        subject_prefix: NatsToken::new("zoom").unwrap(),
        stream_name: NatsToken::new("ZOOM").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    }
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn mock_app(publisher: MockJetStreamPublisher) -> Router {
    router_with_clock(
        wrap_publisher(publisher),
        &test_config(),
        FixedEpochClock::from_secs(TEST_NOW),
    )
}

fn sign(body: &[u8], timestamp: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(test_secret().as_bytes()).unwrap();
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);
    format!("v0={}", hex::encode(mac.finalize().into_bytes()))
}

fn now() -> String {
    TEST_NOW.to_string()
}

fn webhook_request(body: &[u8], timestamp: &str, signature: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_TIMESTAMP, timestamp)
        .header(HEADER_SIGNATURE, signature)
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn signed_request(body: &[u8]) -> Request<Body> {
    let timestamp = now();
    webhook_request(body, &timestamp, &sign(body, &timestamp))
}

fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1, "expected exactly one unroutable publish");
    assert_eq!(messages[0].subject, "zoom.unroutable");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_REJECT_REASON).map(|v| v.as_str()),
        Some(expected_reason),
    );
    assert_eq!(
        messages[0]
            .headers
            .get(NATS_HEADER_REQUEST_TIMESTAMP)
            .map(|v| v.as_str()),
        Some(now().as_str()),
    );
}

#[tokio::test]
async fn provision_creates_stream() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();

    provision(&js, &test_config()).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "ZOOM");
    assert_eq!(streams[0].subjects, vec!["zoom.>"]);
    assert_eq!(streams[0].max_age, Duration::from_secs(3600));
}

#[tokio::test]
async fn valid_transcript_completed_publishes_raw_body_on_dotted_subject() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event":"recording.transcript_completed","event_ts":1700000000000,"payload":{"account_id":"abc"}}"#;

    let resp = app.oneshot(signed_request(body)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "zoom.recording.transcript_completed");
    assert_eq!(messages[0].payload.as_ref(), body);
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT_TYPE).map(|v| v.as_str()),
        Some("recording.transcript_completed"),
    );
    assert_eq!(
        messages[0]
            .headers
            .get(NATS_HEADER_REQUEST_TIMESTAMP)
            .map(|v| v.as_str()),
        Some(now().as_str()),
    );
    assert!(messages[0].headers.get(async_nats::header::NATS_MESSAGE_ID).is_none());
}

#[tokio::test]
async fn other_events_publish_on_their_own_subjects() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();

    for event in ["recording.completed", "meeting.ended"] {
        let body = format!(r#"{{"event":"{event}","payload":{{}}}}"#);
        let resp = mock_app(publisher.clone())
            .oneshot(signed_request(body.as_bytes()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    assert_eq!(
        publisher.published_subjects(),
        vec!["zoom.recording.completed", "zoom.meeting.ended"]
    );
}

#[tokio::test]
async fn url_validation_returns_encrypted_token_without_publishing() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event":"endpoint.url_validation","payload":{"plainToken":"abc123"}}"#;

    let resp = app.oneshot(signed_request(body)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["plainToken"], "abc123");
    assert_eq!(
        json["encryptedToken"],
        signature::encrypt_plain_token(&test_secret(), "abc123").unwrap()
    );
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn url_validation_with_bad_signature_is_rejected() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event":"endpoint.url_validation","payload":{"plainToken":"abc123"}}"#;

    let resp = app.oneshot(webhook_request(body, &now(), "v0=00")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn url_validation_without_plain_token_publishes_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event":"endpoint.url_validation","payload":{}}"#;

    let resp = app.oneshot(signed_request(body)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "missing_plain_token");
}

#[tokio::test]
async fn bad_signature_returns_401_and_publishes_nothing() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event":"meeting.ended"}"#;

    let resp = app.oneshot(webhook_request(body, &now(), "v0=deadbeef")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn signature_over_different_body_is_rejected() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let timestamp = now();
    let signature = sign(br#"{"event":"meeting.started"}"#, &timestamp);

    let resp = app
        .oneshot(webhook_request(br#"{"event":"meeting.ended"}"#, &timestamp, &signature))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn stale_timestamp_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event":"meeting.ended"}"#;
    let timestamp = (TEST_NOW - 301).to_string();

    let resp = app
        .oneshot(webhook_request(body, &timestamp, &sign(body, &timestamp)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn timestamp_at_tolerance_boundary_is_accepted() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event":"meeting.ended"}"#;
    let timestamp = (TEST_NOW - 300).to_string();

    let resp = app
        .oneshot(webhook_request(body, &timestamp, &sign(body, &timestamp)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn future_timestamp_beyond_tolerance_returns_401() {
    let _guard = tracing_guard();
    let app = mock_app(MockJetStreamPublisher::new());
    let body = br#"{"event":"meeting.ended"}"#;
    let timestamp = (TEST_NOW + 301).to_string();

    let resp = app
        .oneshot(webhook_request(body, &timestamp, &sign(body, &timestamp)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_numeric_timestamp_returns_401() {
    let _guard = tracing_guard();
    let app = mock_app(MockJetStreamPublisher::new());
    let body = br#"{"event":"meeting.ended"}"#;

    let resp = app
        .oneshot(webhook_request(body, "yesterday", &sign(body, "yesterday")))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn missing_timestamp_header_returns_401() {
    let _guard = tracing_guard();
    let app = mock_app(MockJetStreamPublisher::new());
    let body = br#"{"event":"meeting.ended"}"#;
    let request = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_SIGNATURE, sign(body, &now()))
        .body(Body::from(body.to_vec()))
        .unwrap();

    let resp = app.oneshot(request).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn missing_signature_header_returns_401() {
    let _guard = tracing_guard();
    let app = mock_app(MockJetStreamPublisher::new());
    let request = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_TIMESTAMP, now())
        .body(Body::from(br#"{"event":"meeting.ended"}"#.to_vec()))
        .unwrap();

    let resp = app.oneshot(request).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn invalid_json_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app.oneshot(signed_request(b"{")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "invalid_json");
}

#[tokio::test]
async fn missing_event_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app.oneshot(signed_request(br#"{"payload":{}}"#)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "missing_event_type");
}

#[tokio::test]
async fn invalid_event_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app
        .oneshot(signed_request(br#"{"event":"meeting ended"}"#))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "invalid_event_type");
}

#[tokio::test]
async fn publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher);

    let resp = app
        .oneshot(signed_request(br#"{"event":"meeting.ended"}"#))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn unroutable_publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher);

    let resp = app.oneshot(signed_request(b"{")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
