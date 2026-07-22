use super::super::config::SentryConfig;
use super::super::constants::{
    HEADER_REQUEST_ID, HEADER_RESOURCE, HEADER_SIGNATURE, HEADER_TIMESTAMP, NATS_HEADER_ACTION, NATS_HEADER_REQUEST_ID,
    NATS_HEADER_RESOURCE, NATS_HEADER_TIMESTAMP,
};
use super::super::sentry_client_secret::SentryClientSecret;
use super::*;
use axum::body::Body;
use axum::http::Request;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};

type HmacSha256 = Hmac<Sha256>;

const TEST_SECRET: &str = "test-secret";

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
    hex::encode(mac.finalize().into_bytes())
}

fn test_config() -> SentryConfig {
    SentryConfig {
        client_secret: SentryClientSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("sentry").unwrap(),
        stream_name: NatsToken::new("SENTRY").unwrap(),
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

fn webhook_request(
    body: &[u8],
    resource: &str,
    timestamp: &str,
    request_id: &str,
    signature: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_RESOURCE, resource)
        .header(HEADER_TIMESTAMP, timestamp)
        .header(HEADER_REQUEST_ID, request_id);

    if let Some(signature) = signature {
        builder = builder.header(HEADER_SIGNATURE, signature);
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
    assert_eq!(streams[0].name, "SENTRY");
    assert_eq!(streams[0].subjects, vec!["sentry.>"]);
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
    let body = br#"{"action":"created","data":{}}"#;
    let signature = compute_sig(TEST_SECRET, body);

    let response = app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some(&signature)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "sentry.issue.created");
    assert_eq!(messages[0].payload.as_ref(), body);
    assert_eq!(
        messages[0]
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .unwrap()
            .as_str(),
        "req-1"
    );
    assert_eq!(messages[0].headers.get(NATS_HEADER_RESOURCE).unwrap().as_str(), "issue");
    assert_eq!(messages[0].headers.get(NATS_HEADER_ACTION).unwrap().as_str(), "created");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_REQUEST_ID).unwrap().as_str(),
        "req-1"
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_TIMESTAMP).unwrap().as_str(),
        "1711315768"
    );
}

#[tokio::test]
async fn missing_signature_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"created"}"#;

    let response = app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-1", None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn invalid_signature_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"created"}"#;

    let response = app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some("not-valid")))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn missing_resource_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"created"}"#;
    let signature = compute_sig(TEST_SECRET, body);

    let request = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_SIGNATURE, signature)
        .body(Body::from(body.to_vec()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn missing_timestamp_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"created"}"#;
    let signature = compute_sig(TEST_SECRET, body);

    let request = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_RESOURCE, "issue")
        .header(HEADER_REQUEST_ID, "req-1")
        .header(HEADER_SIGNATURE, signature)
        .body(Body::from(body.to_vec()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn missing_request_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"created"}"#;
    let signature = compute_sig(TEST_SECRET, body);

    let request = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header(HEADER_RESOURCE, "issue")
        .header(HEADER_TIMESTAMP, "1711315768")
        .header(HEADER_SIGNATURE, signature)
        .body(Body::from(body.to_vec()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn invalid_payload_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"not-json"#;
    let signature = compute_sig(TEST_SECRET, body);

    let response = app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some(&signature)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn unsupported_resource_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"created"}"#;
    let signature = compute_sig(TEST_SECRET, body);

    let response = app
        .oneshot(webhook_request(
            body,
            "organization",
            "1711315768",
            "req-1",
            Some(&signature),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[test]
fn sentry_resource_as_str_covers_all_documented_values() {
    assert_eq!(SentryResource::Installation.as_str(), "installation");
    assert_eq!(SentryResource::EventAlert.as_str(), "event_alert");
    assert_eq!(SentryResource::Issue.as_str(), "issue");
    assert_eq!(SentryResource::MetricAlert.as_str(), "metric_alert");
    assert_eq!(SentryResource::Error.as_str(), "error");
    assert_eq!(SentryResource::Comment.as_str(), "comment");
    assert_eq!(SentryResource::Seer.as_str(), "seer");
}

#[tokio::test]
async fn invalid_action_token_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"action":"issue.created"}"#;
    let signature = compute_sig(TEST_SECRET, body);

    let response = app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some(&signature)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn publish_error_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher);
    let body = br#"{"action":"created"}"#;
    let signature = compute_sig(TEST_SECRET, body);

    let response = app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-1", Some(&signature)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
