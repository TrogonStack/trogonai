use super::*;
use axum::body::Body;
use axum::http::Request;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::time::Duration;
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        MockObjectStore::new(),
        "test-bucket".to_string(),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

const TEST_SECRET: &str = "test-secret";

fn test_config() -> GithubConfig {
    GithubConfig {
        webhook_secret: GitHubWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("github").unwrap(),
        stream_name: NatsToken::new("GITHUB").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    }
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn mock_app(publisher: MockJetStreamPublisher) -> Router {
    router(wrap_publisher(publisher), &test_config())
}

fn webhook_request(body: &[u8], event: &str, delivery: &str, sig: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_EVENT, event)
        .header(HEADER_DELIVERY, delivery);

    if let Some(s) = sig {
        builder = builder.header(HEADER_SIGNATURE, s);
    }

    builder.body(Body::from(body.to_vec())).unwrap()
}

#[tokio::test]
async fn provision_creates_stream() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();
    let config = test_config();

    provision(&js, &config).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "GITHUB");
    assert_eq!(streams[0].subjects, vec!["github.>"]);
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
    let sig = compute_sig(TEST_SECRET, body);

    let resp = app
        .oneshot(webhook_request(body, "push", "del-1", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "github.push");
    assert_eq!(messages[0].payload, Bytes::from(&body[..]));
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT).map(|v| v.as_str()),
        Some("push"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_DELIVERY).map(|v| v.as_str()),
        Some("del-1"),
    );
}

#[tokio::test]
async fn valid_signature_publishes_and_returns_200() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"opened"}"#;
    let sig = compute_sig(TEST_SECRET, body);

    let resp = app
        .oneshot(webhook_request(body, "pull_request", "del-2", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["github.pull_request"]);
}

#[tokio::test]
async fn invalid_signature_returns_401_and_does_not_publish() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app
        .oneshot(webhook_request(b"{}", "push", "del-3", Some("sha256=deadbeef")))
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

    let resp = app
        .oneshot(webhook_request(b"{}", "push", "del-4", None))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn missing_event_header_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = b"{}";
    let sig = compute_sig(TEST_SECRET, body);

    let req = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_SIGNATURE, sig)
        .body(Body::from(&body[..]))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher.clone());
    let body = b"{}";
    let sig = compute_sig(TEST_SECRET, body);

    let resp = app
        .oneshot(webhook_request(body, "push", "del-5", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn subject_uses_configured_prefix() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();

    let state = AppState {
        publisher: wrap_publisher(publisher.clone()),
        webhook_secret: GitHubWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("custom").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
        )
        .with_state(state);

    let body = b"{}";
    let sig = compute_sig(TEST_SECRET, body);

    let resp = app
        .oneshot(webhook_request(body, "issues", "del-7", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["custom.issues"]);
}

#[tokio::test]
async fn empty_body_publishes_successfully() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = b"";
    let sig = compute_sig(TEST_SECRET, body);

    let resp = app
        .oneshot(webhook_request(body, "ping", "del-8", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_payloads(), vec![Bytes::new()]);
}

#[tokio::test]
async fn missing_delivery_header_defaults_to_unknown() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = b"{}";
    let sig = compute_sig(TEST_SECRET, body);

    let req = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_EVENT, "push")
        .header(HEADER_SIGNATURE, sig)
        .body(Body::from(&body[..]))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_DELIVERY).map(|v| v.as_str()),
        Some("unknown"),
    );
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
        webhook_secret: GitHubWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("github").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<AckFailPublisher, MockObjectStore>))
        .with_state(state);

    let body = b"{}";
    let sig = compute_sig(TEST_SECRET, body);

    let resp = app
        .oneshot(webhook_request(body, "push", "del-9", Some(&sig)))
        .await
        .unwrap();

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
        webhook_secret: GitHubWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("github").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_millis(10).unwrap(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<AckFailPublisher, MockObjectStore>))
        .with_state(state);

    let body = b"{}";
    let sig = compute_sig(TEST_SECRET, body);

    let resp = app
        .oneshot(webhook_request(body, "push", "del-10", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
