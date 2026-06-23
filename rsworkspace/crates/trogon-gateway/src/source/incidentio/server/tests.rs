use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
    };

    type HmacSha256 = Hmac<Sha256>;

    fn test_secret() -> String {
        ["whsec_", "dGVzdC1zZWNyZXQ="].concat()
    }

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

    fn test_config() -> IncidentioConfig {
        IncidentioConfig {
            signing_secret: IncidentioSigningSecret::new(test_secret()).unwrap(),
            subject_prefix: NatsToken::new("incidentio").unwrap(),
            stream_name: NatsToken::new("INCIDENTIO").unwrap(),
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

    fn sign(body: &[u8], webhook_id: &str, timestamp: &str) -> String {
        let secret = test_config().signing_secret;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        let signed = format!("{webhook_id}.{timestamp}.").into_bytes();
        mac.update(&signed);
        mac.update(body);
        format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
    }

    fn valid_timestamp() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    }

    fn webhook_request(body: &[u8], webhook_id: &str, timestamp: &str, signature: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("webhook-id", webhook_id)
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    fn assert_unroutable(
        publisher: &MockJetStreamPublisher,
        expected_reason: &str,
        expected_webhook_id: &str,
        expected_timestamp: &str,
    ) {
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1, "expected exactly one unroutable publish");
        assert_eq!(messages[0].subject, "incidentio.unroutable");
        assert_eq!(
            messages[0].headers.get(NATS_HEADER_REJECT_REASON).map(|v| v.as_str()),
            Some(expected_reason),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|v| v.as_str()),
            Some(expected_webhook_id),
        );
        assert_eq!(
            messages[0].headers.get(NATS_HEADER_WEBHOOK_ID).map(|v| v.as_str()),
            Some(expected_webhook_id),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_WEBHOOK_TIMESTAMP)
                .map(|v| v.as_str()),
            Some(expected_timestamp),
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
        assert_eq!(streams[0].name, "INCIDENTIO");
        assert_eq!(streams[0].subjects, vec!["incidentio.>"]);
        assert_eq!(streams[0].duplicate_window, Duration::from_secs(300));
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn valid_public_event_publishes() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"event_type":"public_incident.incident_created_v2","data":{"id":"01ABC"}}"#;
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(
                body,
                "msg-1",
                &timestamp,
                &sign(body, "msg-1", &timestamp),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "incidentio.public_incident.incident_created_v2");
        assert_eq!(messages[0].payload.as_ref(), body);
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|v| v.as_str()),
            Some("msg-1"),
        );
    }

    #[tokio::test]
    async fn valid_private_event_publishes() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"event_type":"private_incident.incident_updated_v2","data":{"id":"01ABC"}}"#;
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(
                body,
                "msg-2",
                &timestamp,
                &sign(body, "msg-2", &timestamp),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["incidentio.private_incident.incident_updated_v2"]
        );
    }

    #[tokio::test]
    async fn invalid_json_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"{";
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(
                body,
                "msg-3",
                &timestamp,
                &sign(body, "msg-3", &timestamp),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_json", "msg-3", &timestamp);
    }

    #[tokio::test]
    async fn missing_event_type_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"data":{"id":"01ABC"}}"#;
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(
                body,
                "msg-4",
                &timestamp,
                &sign(body, "msg-4", &timestamp),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_unroutable(&publisher, "missing_event_type", "msg-4", &timestamp);
    }

    #[tokio::test]
    async fn invalid_event_type_publishes_unroutable_and_returns_ok() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"event_type":"public incident"}"#;
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(
                body,
                "msg-5",
                &timestamp,
                &sign(body, "msg-5", &timestamp),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_event_type", "msg-5", &timestamp);
    }

    #[tokio::test]
    async fn unauthorized_request_returns_401_and_publishes_nothing() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"event_type":"public_incident.incident_created_v2"}"#;
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(body, "msg-6", &timestamp, "v1,wrong"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher);
        let body = br#"{"event_type":"public_incident.incident_created_v2"}"#;
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(
                body,
                "msg-7",
                &timestamp,
                &sign(body, "msg-7", &timestamp),
            ))
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
        let body = br#"{"event_type":"public incident"}"#;
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(
                body,
                "msg-8",
                &timestamp,
                &sign(body, "msg-8", &timestamp),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn headers_include_webhook_metadata() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"event_type":"public_incident.incident_created_v2"}"#;
        let timestamp = valid_timestamp();
        let resp = app
            .oneshot(webhook_request(
                body,
                "msg-9",
                &timestamp,
                &sign(body, "msg-9", &timestamp),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        let headers = &messages[0].headers;
        assert_eq!(
            headers.get(NATS_HEADER_EVENT_TYPE).map(|v| v.as_str()),
            Some("public_incident.incident_created_v2"),
        );
        assert_eq!(headers.get(NATS_HEADER_WEBHOOK_ID).map(|v| v.as_str()), Some("msg-9"),);
        assert_eq!(
            headers.get(NATS_HEADER_WEBHOOK_TIMESTAMP).map(|v| v.as_str()),
            Some(timestamp.as_str()),
        );
    }
