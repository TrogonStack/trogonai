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

    fn wrap_publisher(
        publisher: MockJetStreamPublisher,
    ) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
        ClaimCheckPublisher::new(
            publisher,
            MockObjectStore::new(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(usize::MAX),
        )
    }

    const TEST_SECRET: &str = "test-secret";

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("valid key length");
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn test_config() -> LinearConfig {
        LinearConfig {
            webhook_secret: LinearWebhookSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("linear").expect("valid token"),
            stream_name: NatsToken::new("LINEAR").expect("valid token"),
            stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
            timestamp_tolerance: NonZeroDuration::from_secs(60).ok(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        }
    }

    fn tracing_guard() -> tracing::subscriber::DefaultGuard {
        tracing_subscriber::fmt().with_test_writer().set_default()
    }

    fn mock_app(publisher: MockJetStreamPublisher) -> Router {
        router(wrap_publisher(publisher), &test_config())
    }

    fn webhook_request(body: &[u8], sig: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/webhook");

        if let Some(s) = sig {
            builder = builder.header("linear-signature", s);
        }

        builder.body(Body::from(body.to_vec())).expect("valid request")
    }

    fn valid_body() -> Vec<u8> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;

        serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create",
            "webhookId": "wh_test_123",
            "webhookTimestamp": now_ms,
            "data": {}
        }))
        .expect("valid JSON")
    }

    fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1, "expected exactly one unroutable publish");
        assert_eq!(messages[0].subject, "linear.unroutable");
        assert_eq!(
            messages[0].headers.get(NATS_HEADER_REJECT_REASON).map(|v| v.as_str()),
            Some(expected_reason),
        );
    }

    fn assert_no_publishes(publisher: &MockJetStreamPublisher) {
        assert!(publisher.published_messages().is_empty(), "expected no publishes");
    }

    #[tokio::test]
    async fn valid_webhook_returns_200() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_body();
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "linear.Issue.create");
    }

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "LINEAR");
        assert_eq!(streams[0].subjects, vec!["linear.>"]);
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
    async fn missing_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_body();

        let resp = app
            .oneshot(webhook_request(&body, None))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn invalid_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_body();

        let resp = app
            .oneshot(webhook_request(&body, Some("bad-sig")))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn invalid_json_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"not json";
        let sig = compute_sig(TEST_SECRET, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_json");
    }

    #[tokio::test]
    async fn missing_webhook_timestamp_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create"
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_webhook_timestamp");
    }

    #[tokio::test]
    async fn stale_webhook_timestamp_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create",
            "webhookTimestamp": 1_000_000_000_000_u64
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "stale_webhook_timestamp");
    }

    #[tokio::test]
    async fn missing_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "action": "create",
            "webhookTimestamp": now_ms
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_type");
    }

    #[tokio::test]
    async fn invalid_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "has space",
            "action": "create",
            "webhookTimestamp": now_ms
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_type");
    }

    #[tokio::test]
    async fn missing_action_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "webhookTimestamp": now_ms
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_action");
    }

    #[tokio::test]
    async fn invalid_action_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "has.dot",
            "webhookTimestamp": now_ms
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_action");
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = valid_body();
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn dlq_publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = b"not json";
        let sig = compute_sig(TEST_SECRET, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn timestamp_tolerance_disabled_skips_check() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let mut config = test_config();
        config.timestamp_tolerance = None;
        let app = router(wrap_publisher(publisher.clone()), &config);

        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create",
            "webhookId": "wh_no_ts",
            "webhookTimestamp": 1_000_000_000_000_u64
        }))
        .expect("valid JSON");
        let sig = compute_sig(TEST_SECRET, &body);

        let resp = app
            .oneshot(webhook_request(&body, Some(&sig)))
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "linear.Issue.create");
    }
