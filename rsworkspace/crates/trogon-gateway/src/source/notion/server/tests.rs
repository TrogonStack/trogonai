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

    const TEST_VERIFICATION_TOKEN: &str = "notion-verification-token-example";

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

    fn compute_sig(secret: &NotionVerificationToken, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_str().as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn test_config() -> NotionConfig {
        NotionConfig {
            verification_token: NotionVerificationToken::new(TEST_VERIFICATION_TOKEN).unwrap(),
            subject_prefix: NatsToken::new("notion").unwrap(),
            stream_name: NatsToken::new("NOTION").unwrap(),
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

    fn webhook_request(body: &[u8], signature: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/webhook");

        if let Some(signature) = signature {
            builder = builder.header(HEADER_SIGNATURE, signature);
        }

        builder.body(Body::from(body.to_vec())).unwrap()
    }

    fn valid_event_body() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "id": "367cba44-b6f3-4c92-81e7-6a2e9659efd4",
            "subscription_id": "29d75c0d-5546-4414-8459-7b7a92f1fc4b",
            "attempt_number": 1,
            "type": "page.created",
            "entity": {
                "id": "153104cd-477e-809d-8dc4-ff2d96ae3090",
                "type": "page"
            }
        }))
        .unwrap()
    }

    fn verification_body() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "verification_token": TEST_VERIFICATION_TOKEN
        }))
        .unwrap()
    }

    fn assert_no_publishes(publisher: &MockJetStreamPublisher) {
        assert!(publisher.published_messages().is_empty());
    }

    fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "notion.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|value| value.as_str()),
            Some(expected_reason),
        );
    }

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "NOTION");
        assert_eq!(streams[0].subjects, vec!["notion.>"]);
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn valid_event_publishes_to_nats() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_event_body();
        let signature = compute_sig(&test_config().verification_token, &body);

        let response = app.oneshot(webhook_request(&body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "notion.page.created");
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|value| value.as_str()),
            Some("367cba44-b6f3-4c92-81e7-6a2e9659efd4"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT_TYPE)
                .map(|value| value.as_str()),
            Some("page.created"),
        );
    }

    #[tokio::test]
    async fn bootstrap_verification_request_without_signature_is_accepted() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = verification_body();

        let response = app.oneshot(webhook_request(&body, None)).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(publisher.published_subjects(), vec!["notion.subscription.verification"]);
    }

    #[tokio::test]
    async fn verification_request_with_bad_signature_is_rejected() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = verification_body();

        let response = app.oneshot(webhook_request(&body, Some("sha256=bad"))).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn verification_request_with_empty_token_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "verification_token": ""
        }))
        .unwrap();

        let response = app.oneshot(webhook_request(&body, None)).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn missing_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = valid_event_body();

        let response = app.oneshot(webhook_request(&body, None)).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_no_publishes(&publisher);
    }

    #[tokio::test]
    async fn invalid_json_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"not-json";
        let signature = compute_sig(&test_config().verification_token, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_json");
    }

    #[tokio::test]
    async fn missing_event_type_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "id": "367cba44-b6f3-4c92-81e7-6a2e9659efd4"
        }))
        .unwrap();
        let signature = compute_sig(&test_config().verification_token, &body);

        let response = app.oneshot(webhook_request(&body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "missing_event_type");
    }

    #[tokio::test]
    async fn invalid_event_type_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "id": "367cba44-b6f3-4c92-81e7-6a2e9659efd4",
            "type": "page created"
        }))
        .unwrap();
        let signature = compute_sig(&test_config().verification_token, &body);

        let response = app.oneshot(webhook_request(&body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_event_type");
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher);
        let body = valid_event_body();
        let signature = compute_sig(&test_config().verification_token, &body);

        let response = app.oneshot(webhook_request(&body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
