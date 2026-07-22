use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};

const TEST_TOKEN: &str = "datadog-webhook-token";
const DEFAULT_HEADER: &str = "x-datadog-webhook-token";

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        MockObjectStore::new(),
        "test-bucket".to_string(),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn test_config() -> DatadogConfig {
    config_with_tolerance(None)
}

fn config_with_tolerance(timestamp_tolerance: Option<NonZeroDuration>) -> DatadogConfig {
    DatadogConfig {
        webhook_token: DatadogWebhookToken::new(TEST_TOKEN).unwrap(),
        subject_prefix: NatsToken::new("datadog").unwrap(),
        stream_name: NatsToken::new("DATADOG").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance,
        webhook_token_header: HeaderName::from_static(DEFAULT_HEADER),
    }
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn mock_app(publisher: MockJetStreamPublisher) -> Router {
    router(wrap_publisher(publisher), &test_config())
}

fn mock_app_with_tolerance(publisher: MockJetStreamPublisher, tolerance_secs: u64) -> Router {
    let config = config_with_tolerance(NonZeroDuration::from_secs(tolerance_secs).ok());
    router(wrap_publisher(publisher), &config)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn webhook_request(body: &[u8], token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri("/webhook");
    if let Some(token) = token {
        builder = builder.header(DEFAULT_HEADER, token);
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
    assert_eq!(streams[0].name, "DATADOG");
    assert_eq!(streams[0].subjects, vec!["datadog.>"]);
    assert_eq!(streams[0].max_age, Duration::from_secs(3600));
}

#[tokio::test]
async fn valid_event_publishes() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event_type":"metric_alert_monitor","id":"7890","title":"CPU high"}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "datadog.metric_alert_monitor");
    assert_eq!(messages[0].payload.as_ref(), body);
    assert_eq!(
        messages[0]
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .map(|v| v.as_str()),
        Some("7890"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT_TYPE).map(|v| v.as_str()),
        Some("metric_alert_monitor"),
    );
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_EVENT_ID).map(|v| v.as_str()),
        Some("7890"),
    );
}

#[tokio::test]
async fn valid_event_without_id_publishes_without_message_id() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event_type":"event_alert"}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "datadog.event_alert");
    assert!(messages[0].headers.get(async_nats::header::NATS_MESSAGE_ID).is_none());
    assert!(messages[0].headers.get(NATS_HEADER_EVENT_ID).is_none());
}

#[tokio::test]
async fn invalid_json_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = b"{";

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "datadog.unroutable");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_REJECT_REASON).map(|v| v.as_str()),
        Some("invalid_json"),
    );
    assert!(messages[0].headers.get(async_nats::header::NATS_MESSAGE_ID).is_none());
}

#[tokio::test]
async fn missing_event_type_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"id":"42","title":"no type"}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "datadog.unroutable");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_REJECT_REASON).map(|v| v.as_str()),
        Some("missing_event_type"),
    );
    assert_eq!(
        messages[0]
            .headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .map(|v| v.as_str()),
        Some("42"),
    );
}

#[tokio::test]
async fn invalid_event_type_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event_type":"metric alert","id":"42"}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "datadog.unroutable");
    assert_eq!(
        messages[0].headers.get(NATS_HEADER_REJECT_REASON).map(|v| v.as_str()),
        Some("invalid_event_type"),
    );
}

#[tokio::test]
async fn missing_token_returns_401_and_publishes_nothing() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event_type":"event_alert"}"#;

    let resp = app.oneshot(webhook_request(body, None)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn wrong_token_returns_401_and_publishes_nothing() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event_type":"event_alert"}"#;

    let resp = app.oneshot(webhook_request(body, Some("wrong-token"))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher);
    let body = br#"{"event_type":"event_alert","id":"42"}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn unroutable_publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let app = mock_app(publisher);
    let body = br#"{"event_type":"metric alert"}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn fresh_timestamp_publishes_when_tolerance_enabled() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app_with_tolerance(publisher.clone(), 300);
    let body = format!(r#"{{"event_type":"event_alert","timestamp":{}}}"#, now_secs()).into_bytes();

    let resp = app.oneshot(webhook_request(&body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["datadog.event_alert"]);
}

#[tokio::test]
async fn fresh_timestamp_as_string_publishes() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app_with_tolerance(publisher.clone(), 300);
    let body = format!(r#"{{"event_type":"event_alert","timestamp":"{}"}}"#, now_secs()).into_bytes();

    let resp = app.oneshot(webhook_request(&body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["datadog.event_alert"]);
}

#[tokio::test]
async fn stale_timestamp_returns_401_and_publishes_nothing() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app_with_tolerance(publisher.clone(), 300);
    let stale = now_secs() - 10_000;
    let body = format!(r#"{{"event_type":"event_alert","timestamp":{stale}}}"#).into_bytes();

    let resp = app.oneshot(webhook_request(&body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn missing_timestamp_returns_400_when_tolerance_enabled() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app_with_tolerance(publisher.clone(), 300);
    let body = br#"{"event_type":"event_alert"}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn timestamp_ignored_when_tolerance_disabled() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let app = mock_app(publisher.clone());
    let body = br#"{"event_type":"event_alert","timestamp":1}"#;

    let resp = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["datadog.event_alert"]);
}

#[tokio::test]
async fn custom_webhook_token_header_is_honored() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let config = DatadogConfig {
        webhook_token_header: HeaderName::from_static("x-acme-token"),
        ..test_config()
    };
    let app = router(wrap_publisher(publisher.clone()), &config);
    let body = br#"{"event_type":"event_alert"}"#;

    let custom = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header("x-acme-token", TEST_TOKEN)
        .body(Body::from(body.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(custom).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_subjects(), vec!["datadog.event_alert"]);

    let default_header = app.oneshot(webhook_request(body, Some(TEST_TOKEN))).await.unwrap();
    assert_eq!(default_header.status(), StatusCode::UNAUTHORIZED);
}
