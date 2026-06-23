    use super::*;
    use async_nats::jetstream::publish::PublishAck;
    use async_nats::subject::ToSubject;
    use axum::{body::Body, http::Request};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    use std::future::{Future, IntoFuture};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::StreamMaxAge;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, JetStreamPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher,
        MockObjectStore,
    };
    use trogon_nats::mocks::MockError;

    type HmacSha256 = Hmac<Sha256>;

    const TEST_SECRET: &str = "test-secret";

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

    fn test_config() -> TwitterConfig {
        TwitterConfig {
            consumer_secret: TwitterConsumerSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("twitter").unwrap(),
            stream_name: NatsToken::new("TWITTER").unwrap(),
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

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", STANDARD.encode(mac.finalize().into_bytes()))
    }

    fn webhook_request(body: &[u8], signature: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/webhook");
        if let Some(signature) = signature {
            builder = builder.header(HEADER_SIGNATURE, signature);
        }
        builder.body(Body::from(body.to_vec())).unwrap()
    }

    fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|value| value.as_str()),
            Some(expected_reason),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|value| value.as_str()),
            Some("unroutable"),
        );
    }

    mod ack_test_support;


    use ack_test_support::AckFailPublisher;

    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "TWITTER");
        assert_eq!(streams[0].subjects, vec!["twitter.>"]);
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn crc_request_returns_response_token() {
        let _guard = tracing_guard();
        let app = mock_app(MockJetStreamPublisher::new());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/webhook?crc_token=challenge")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            payload["response_token"],
            signature::crc_response_token(TEST_SECRET, "challenge").unwrap(),
        );
    }

    #[tokio::test]
    async fn crc_request_without_token_returns_bad_request() {
        let _guard = tracing_guard();
        let app = mock_app(MockJetStreamPublisher::new());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/webhook")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn x_activity_event_publishes_to_event_type_subject() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "data": {
                "event_type": "profile.update.bio",
                "payload": {
                    "before": "Mars & Cars",
                    "after": "Mars, Cars & AI"
                }
            }
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.profile.update.bio");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT_TYPE)
                .map(|value| value.as_str()),
            Some("profile.update.bio"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|value| value.as_str()),
            Some("x_activity"),
        );
    }

    #[tokio::test]
    async fn account_activity_event_publishes_to_event_bucket_subject() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "for_user_id": "2244994945",
            "favorite_events": [{
                "id": "fav-1",
                "created_at": "Mon Mar 26 16:33:26 +0000 2018"
            }]
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.favorite_events");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|value| value.as_str()),
            Some("account_activity"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|value| value.as_str()),
            Some("favorite_events:fav-1"),
        );
    }

    #[tokio::test]
    async fn generic_account_activity_fallback_omits_message_id() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "for_user_id": "2244994945",
            "favorite_events": [{
                "id": "fav-1"
            }],
            "follow_events": [{
                "id": "follow-1"
            }]
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.account_activity");
        assert_eq!(messages[0].headers.get(async_nats::header::NATS_MESSAGE_ID), None);
    }

    #[tokio::test]
    async fn filtered_stream_payload_publishes_to_filtered_stream_subject() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "data": {
                "id": "1234567890",
                "text": "hello world"
            },
            "matching_rules": [
                { "id": "rule-1", "tag": "cats" }
            ]
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter.filtered_stream");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|value| value.as_str()),
            Some("filtered_stream"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|value| value.as_str()),
            Some("filtered_stream:1234567890"),
        );
    }

    #[tokio::test]
    async fn invalid_signature_returns_unauthorized() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let response = app
            .oneshot(webhook_request(
                br#"{"data":{"event_type":"profile.update.bio"}}"#,
                Some("sha256=deadbeef"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn invalid_json_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"{not-json";
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_json");
    }

    #[tokio::test]
    async fn unsupported_shape_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"hello":"world"}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "missing_event_type");
    }

    #[tokio::test]
    async fn invalid_event_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{
            "data": {
                "event_type": "invalid token"
            }
        }"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_unroutable(&publisher, "invalid_event_type");
    }

    #[tokio::test]
    async fn publish_ack_failure_returns_internal_server_error() {
        let _guard = tracing_guard();
        let publisher = AckFailPublisher::failing();

        let state = AppState {
            publisher: ClaimCheckPublisher::new(
                publisher,
                MockObjectStore::new(),
                "test-bucket".to_string(),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            consumer_secret: TwitterConsumerSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("twitter").unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        };

        let app = Router::new()
            .route(
                "/webhook",
                get(handle_crc::<AckFailPublisher, MockObjectStore>)
                    .post(handle_webhook::<AckFailPublisher, MockObjectStore>),
            )
            .with_state(state);

        let body = br#"{"data":{"event_type":"profile.update.bio"}}"#;
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn unroutable_publish_ack_failure_returns_internal_server_error() {
        let _guard = tracing_guard();
        let publisher = AckFailPublisher::failing();

        let state = AppState {
            publisher: ClaimCheckPublisher::new(
                publisher,
                MockObjectStore::new(),
                "test-bucket".to_string(),
                MaxPayload::from_server_limit(usize::MAX),
            ),
            consumer_secret: TwitterConsumerSecret::new(TEST_SECRET).unwrap(),
            subject_prefix: NatsToken::new("twitter").unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        };

        let app = Router::new()
            .route(
                "/webhook",
                get(handle_crc::<AckFailPublisher, MockObjectStore>)
                    .post(handle_webhook::<AckFailPublisher, MockObjectStore>),
            )
            .with_state(state);

        let body = b"{not-json";
        let signature = compute_sig(TEST_SECRET, body);

        let response = app.oneshot(webhook_request(body, Some(&signature))).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn resolve_event_type_rejects_non_object_payloads() {
        let payload = serde_json::json!(42);

        assert_eq!(resolve_event_type(&payload), Err(RejectReason::MissingEventType));
    }

    #[test]
    fn resolve_event_type_falls_back_for_multiple_account_activity_keys() {
        let payload = serde_json::json!({
            "favorite_events": [{"id": "fav-1"}],
            "follow_events": [{"id": "follow-1"}]
        });

        let (event_type, payload_kind) = resolve_event_type(&payload).expect("event type");
        assert_eq!(event_type.as_str(), "account_activity");
        assert_eq!(payload_kind, PayloadKind::AccountActivity);
    }

    #[test]
    fn extract_message_id_uses_object_payload_ids() {
        let payload = serde_json::json!({
            "user_event": {
                "id": "user-event-1"
            }
        });

        assert_eq!(
            extract_message_id(&payload, "user_event").as_deref(),
            Some("user_event:user-event-1")
        );
    }

    #[test]
    fn extract_message_id_skips_multi_item_account_activity_arrays() {
        let payload = serde_json::json!({
            "favorite_events": [
                {"id": "fav-1"},
                {"id": "fav-2"}
            ]
        });

        assert_eq!(extract_message_id(&payload, "favorite_events"), None);
    }

    #[test]
    fn extract_message_id_omits_generic_account_activity_bucket() {
        let payload = serde_json::json!({
            "favorite_events": [{"id": "fav-1"}],
            "follow_events": [{"id": "follow-1"}]
        });

        assert_eq!(extract_message_id(&payload, "account_activity"), None);
    }

    #[test]
    fn extract_message_id_prefixes_data_ids_with_event_type() {
        let payload = serde_json::json!({
            "data": {
                "id": "1234567890"
            }
        });

        assert_eq!(
            extract_message_id(&payload, "filtered_stream").as_deref(),
            Some("filtered_stream:1234567890")
        );
    }

    #[test]
    fn value_id_supports_numeric_ids() {
        let value = serde_json::json!({
            "id": 12345
        });

        assert_eq!(value_id(&value).as_deref(), Some("12345"));
    }

    #[test]
    fn value_id_rejects_non_string_and_non_numeric_ids() {
        let value = serde_json::json!({
            "id": true
        });

        assert_eq!(value_id(&value), None);
    }
