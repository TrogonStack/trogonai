use super::*;
use axum::body::Body;
use axum::http::Request;
use std::time::Duration;
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};
use trogon_std::NonZeroDuration;

const TEST_SECRET: &str = "test-secret";

fn test_config() -> TelegramSourceConfig {
    TelegramSourceConfig {
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        registration: None,
        subject_prefix: NatsToken::new("telegram").unwrap(),
        stream_name: NatsToken::new("TELEGRAM").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    }
}

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        MockObjectStore::new(),
        "test-bucket".to_string(),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn mock_app(publisher: MockJetStreamPublisher) -> Router {
    router(wrap_publisher(publisher), &test_config())
}

fn webhook_request(body: &[u8], secret: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri("/webhook");

    if let Some(s) = secret {
        builder = builder.header(HEADER_SECRET_TOKEN, s);
    }

    builder.body(Body::from(body.to_vec())).unwrap()
}

fn update_json(update_type: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "update_id": 12345,
        update_type: {"chat": {"id": 1}}
    }))
    .unwrap()
}

#[tokio::test]
async fn provision_creates_stream() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();
    let config = test_config();

    provision(&js, &config).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "TELEGRAM");
    assert_eq!(streams[0].subjects, vec!["telegram.>"]);
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
async fn valid_message_update_publishes_to_nats() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "telegram.message");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_UPDATE_TYPE).map(|v| v.as_str()),
        Some("message"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_UPDATE_ID).map(|v| v.as_str()),
        Some("12345"),
    );
}

#[tokio::test]
async fn callback_query_routes_to_correct_subject() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = update_json("callback_query");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["telegram.callback_query"]);
}

#[tokio::test]
async fn unknown_update_type_routes_to_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"update_id": 99}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["telegram.unroutable"]);
}

#[tokio::test]
async fn missing_update_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"message": {"chat": {"id": 1}}}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn non_integer_update_id_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"update_id": "not_a_number", "message": {"chat": {"id": 1}}}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn missing_secret_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app.oneshot(webhook_request(b"{}", None)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn wrong_secret_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app.oneshot(webhook_request(b"{}", Some("wrong-secret"))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn invalid_json_returns_400() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());

    let resp = app
        .oneshot(webhook_request(b"not-json", Some(TEST_SECRET)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_subjects().is_empty());
}

#[tokio::test]
async fn publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher.clone());
    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn subject_uses_configured_prefix() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();

    let state = AppState {
        publisher: wrap_publisher(publisher.clone()),
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("custom").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
        )
        .with_state(state);

    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["custom.message"]);
}

#[tokio::test]
async fn update_id_used_as_nats_message_id() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let messages = publisher.published_messages();
    assert_eq!(
        messages[0]
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .map(|v| v.as_str()),
        Some("12345"),
    );
}

#[tokio::test]
async fn body_exceeding_limit_returns_413() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();

    let state = AppState {
        publisher: wrap_publisher(publisher.clone()),
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("telegram").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };

    let app = Router::new()
        .route(
            "/webhook",
            post(handle_webhook::<MockJetStreamPublisher, MockObjectStore>),
        )
        .layer(DefaultBodyLimit::max(64))
        .with_state(state);

    let oversized_body = vec![0u8; 128];
    let resp = app
        .oneshot(webhook_request(&oversized_body, Some(TEST_SECRET)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(publisher.published_messages().is_empty());
}

#[test]
fn extract_update_type_finds_known_types() {
    for &t in UPDATE_TYPES {
        let val = serde_json::json!({"update_id": 1, t: {}});
        assert_eq!(extract_update_type(&val), Some(t), "failed for type: {t}");
    }
}

#[test]
fn extract_update_type_returns_none_for_unknown() {
    let val = serde_json::json!({"update_id": 1});
    assert_eq!(extract_update_type(&val), None);
}

#[test]
fn extract_update_type_returns_none_for_non_object() {
    let val = serde_json::json!("not an object");
    assert_eq!(extract_update_type(&val), None);
}

mod ack_test_support;

use ack_test_support::AckFailPublisher;

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
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("telegram").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<AckFailPublisher, MockObjectStore>))
        .with_state(state);

    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

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
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("telegram").unwrap(),
        nats_ack_timeout: NonZeroDuration::from_millis(10).unwrap(),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<AckFailPublisher, MockObjectStore>))
        .with_state(state);

    let body = update_json("message");

    let resp = app.oneshot(webhook_request(&body, Some(TEST_SECRET))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
