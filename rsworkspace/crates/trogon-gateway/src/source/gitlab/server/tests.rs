use super::*;
use std::time::Duration;

use crate::source::gitlab::constants::{HEADER_WEBHOOK_ID, HEADER_WEBHOOK_SIGNATURE, HEADER_WEBHOOK_TIMESTAMP};
use crate::source::standard_webhooks::sign_for_test;
use axum::body::Body;
use axum::http::Request;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};

const TEST_SIGNING_TOKEN: &str = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=";

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        MockObjectStore::new(),
        "test-bucket".to_string(),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn test_config() -> GitlabConfig {
    GitlabConfig {
        signing_token: GitLabSigningToken::new(TEST_SIGNING_TOKEN).unwrap(),
        subject_prefix: NatsToken::new("gitlab").unwrap(),
        stream_name: NatsToken::new("GITLAB").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    }
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn mock_app(publisher: MockJetStreamPublisher) -> Router {
    router(wrap_publisher(publisher), &test_config())
}

fn valid_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}

fn signing_token() -> GitLabSigningToken {
    GitLabSigningToken::new(TEST_SIGNING_TOKEN).unwrap()
}

fn sign(webhook_id: &str, webhook_timestamp: &str, body: &[u8]) -> String {
    let token = signing_token();
    sign_for_test(token.as_bytes(), webhook_id, webhook_timestamp, body)
}

fn request_builder(event: Option<&str>) -> axum::http::request::Builder {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_WEBHOOK_UUID, "wh-uuid-test")
        .header(HEADER_IDEMPOTENCY_KEY, "idem-key-test")
        .header(HEADER_INSTANCE, "https://gitlab.example.com")
        .header(HEADER_EVENT_UUID, "evt-uuid-test");

    if let Some(event) = event {
        builder = builder.header(HEADER_EVENT, event);
    }

    builder
}

fn webhook_request(body: &[u8], event: &str) -> Request<Body> {
    let webhook_id = "msg_123";
    let timestamp = valid_timestamp();
    request_builder(Some(event))
        .header(HEADER_WEBHOOK_ID, webhook_id)
        .header(HEADER_WEBHOOK_TIMESTAMP, &timestamp)
        .header(HEADER_WEBHOOK_SIGNATURE, sign(webhook_id, &timestamp, body))
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn unsigned_webhook_request(body: &[u8], event: &str) -> Request<Body> {
    request_builder(Some(event)).body(Body::from(body.to_vec())).unwrap()
}

fn webhook_request_with_signature(body: &[u8], event: &str, signature: &str) -> Request<Body> {
    let webhook_id = "msg_123";
    let timestamp = valid_timestamp();
    request_builder(Some(event))
        .header(HEADER_WEBHOOK_ID, webhook_id)
        .header(HEADER_WEBHOOK_TIMESTAMP, &timestamp)
        .header(HEADER_WEBHOOK_SIGNATURE, signature)
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn signed_request_without_event(body: &[u8]) -> Request<Body> {
    let webhook_id = "msg_123";
    let timestamp = valid_timestamp();
    request_builder(None)
        .header(HEADER_WEBHOOK_ID, webhook_id)
        .header(HEADER_WEBHOOK_TIMESTAMP, &timestamp)
        .header(HEADER_WEBHOOK_SIGNATURE, sign(webhook_id, &timestamp, body))
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn signed_request_without_idempotency_key(body: &[u8], event: &str) -> Request<Body> {
    let webhook_id = "msg_123";
    let timestamp = valid_timestamp();
    let signature = sign(webhook_id, &timestamp, body);
    Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_EVENT, event)
        .header(HEADER_WEBHOOK_ID, webhook_id)
        .header(HEADER_WEBHOOK_TIMESTAMP, timestamp)
        .header(HEADER_WEBHOOK_SIGNATURE, signature)
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn webhook_request_with_stale_timestamp(body: &[u8], event: &str) -> Request<Body> {
    let webhook_id = "msg_123";
    let timestamp = "1";
    request_builder(Some(event))
        .header(HEADER_WEBHOOK_ID, webhook_id)
        .header(HEADER_WEBHOOK_TIMESTAMP, timestamp)
        .header(HEADER_WEBHOOK_SIGNATURE, sign(webhook_id, timestamp, body))
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn request_with_legacy_token(body: &[u8], event: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_EVENT, event)
        .header("x-gitlab-token", "test-secret")
        .body(Body::from(body.to_vec()))
        .unwrap()
}

#[test]
fn reject_reason_as_str() {
    assert_eq!(RejectReason::MissingEventHeader.as_str(), "missing_event_header");
    assert_eq!(RejectReason::InvalidEventToken.as_str(), "invalid_event_token");
}

#[tokio::test]
async fn provision_creates_stream() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();
    let config = test_config();

    provision(&js, &config).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "GITLAB");
    assert_eq!(streams[0].subjects, vec!["gitlab.>"]);
    assert_eq!(streams[0].max_age, Duration::from_secs(3600));
}

#[tokio::test]
async fn provision_propagates_error() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();
    js.fail_next();
    let config = test_config();

    let result = provision(&js, &config).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn valid_webhook_publishes_to_nats_and_returns_200() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"ref":"refs/heads/main"}"#;
    let resp = app.oneshot(webhook_request(body, "push")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "gitlab.push");
    assert_eq!(messages[0].payload, Bytes::from(&body[..]));
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT).map(|v| v.as_str()),
        Some("push"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_WEBHOOK_UUID).map(|v| v.as_str()),
        Some("wh-uuid-test"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_INSTANCE).map(|v| v.as_str()),
        Some("https://gitlab.example.com"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT_UUID).map(|v| v.as_str()),
        Some("evt-uuid-test"),
    );
    assert_eq!(
        messages[0]
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .map(|v| v.as_str()),
        Some("idem-key-test"),
    );
}

#[tokio::test]
async fn valid_signature_publishes_and_returns_200() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"opened"}"#;
    let resp = app.oneshot(webhook_request(body, "pull_request")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["gitlab.pull_request"]);
}

#[tokio::test]
async fn invalid_signature_returns_401_and_does_not_publish() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app
        .oneshot(webhook_request_with_signature(b"{}", "push", "v1,d3Jvbmc="))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn missing_signature_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app.oneshot(unsigned_webhook_request(b"{}", "push")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn stale_signature_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app
        .oneshot(webhook_request_with_stale_timestamp(b"{}", "push"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn legacy_secret_token_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app.oneshot(request_with_legacy_token(b"{}", "push")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn missing_event_header_publishes_to_dlq() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = b"{}";
    let req = signed_request_without_event(body);

    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "gitlab.unroutable");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_REJECT_REASON).map(|v| v.as_str()),
        Some("missing_event_header"),
    );
}

#[tokio::test]
async fn publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher.clone());
    let body = b"{}";
    let resp = app.oneshot(webhook_request(body, "push")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn subject_uses_configured_prefix() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();

    let state = AppState {
        publisher: wrap_publisher(publisher.clone()),
        signing_token: signing_token(),
        subject_prefix: NatsToken::new("custom").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
        )
        .with_state(state);

    let body = b"{}";
    let resp = app.oneshot(webhook_request(body, "issues")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["custom.issues"]);
}

#[tokio::test]
async fn empty_body_publishes_successfully() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = b"";
    let resp = app.oneshot(webhook_request(body, "ping")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_payloads(), vec![Bytes::new()]);
}

#[tokio::test]
async fn missing_idempotency_key_skips_dedup_id() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();

    let state = AppState {
        publisher: wrap_publisher(publisher.clone()),
        signing_token: signing_token(),
        subject_prefix: NatsToken::new("gitlab").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
        )
        .with_state(state);

    let req = signed_request_without_idempotency_key(b"{}", "push");

    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert!(
        messages[0].headers.get(async_nats::header::NATS_MESSAGE_ID).is_none(),
        "should not set Nats-Msg-Id when Idempotency-Key is absent"
    );
}

#[tokio::test]
async fn event_with_spaces_normalizes_to_underscores() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = b"{}";
    let resp = app.oneshot(webhook_request(body, "Merge Request Hook")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["gitlab.merge_request_hook"]);
}

#[tokio::test]
async fn dlq_publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher.clone());
    let body = b"{}";
    let req = signed_request_without_event(body);

    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

mod ack_test_support;

use ack_test_support::AckFailPublisher;
use async_nats::subject::ToSubject;

#[tokio::test]
async fn ack_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = AckFailPublisher::failing();

    let state = AppState {
        publisher: ClaimCheckPublisher::new(
            publisher,
            MockObjectStore::new(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        signing_token: signing_token(),
        subject_prefix: NatsToken::new("gitlab").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<AckFailPublisher, MockObjectStore>))
        .with_state(state);

    let body = b"{}";
    let resp = app.oneshot(webhook_request(body, "push")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn ack_timeout_returns_500() {
    let _guard = tracing_guard();
    let publisher = AckFailPublisher::hanging();

    let state = AppState {
        publisher: ClaimCheckPublisher::new(
            publisher,
            MockObjectStore::new(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(usize::MAX),
        ),
        signing_token: signing_token(),
        subject_prefix: NatsToken::new("gitlab").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_millis(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<AckFailPublisher, MockObjectStore>))
        .with_state(state);

    let body = b"{}";
    let resp = app.oneshot(webhook_request(body, "push")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
