use super::config::TelegramSourceConfig;
use super::config::TelegramWebhookSecret;
use super::constants::{
    HEADER_SECRET_TOKEN, HTTP_BODY_SIZE_MAX, NATS_HEADER_UPDATE_ID, NATS_HEADER_UPDATE_TYPE, UPDATE_TYPES,
};
use super::signature;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};
use trogon_std::NonZeroDuration;

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Telegram update to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("telegram");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    webhook_secret: TelegramWebhookSecret,
    subject_prefix: NatsToken,
    nats_ack_timeout: NonZeroDuration,
}

pub async fn provision<C: JetStreamContext>(js: &C, config: &TelegramSourceConfig) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.as_str().to_owned(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age.into(),
        ..Default::default()
    })
    .await?;

    let stream = config.stream_name.as_str();
    let max_age_secs = Duration::from(config.stream_max_age).as_secs();
    info!(stream, max_age_secs, "JetStream stream ready");
    Ok(())
}

pub fn router<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &TelegramSourceConfig,
) -> Router {
    let state = AppState {
        publisher,
        webhook_secret: config.webhook_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> Pin<Box<dyn Future<Output = StatusCode> + Send>> {
    Box::pin(handle_webhook_inner(state, headers, body))
}

/// Extracts the update type by finding the first known field present in the JSON.
///
/// Telegram `Update` objects contain `update_id` plus exactly one optional field
/// that identifies the update type (e.g. `message`, `callback_query`).
fn extract_update_type(value: &serde_json::Value) -> Option<&'static str> {
    let obj = value.as_object()?;
    UPDATE_TYPES.iter().find(|&&t| obj.contains_key(t)).copied()
}

#[instrument(
    name = "telegram.webhook",
    skip_all,
    fields(
        update_type = tracing::field::Empty,
        update_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let token = headers.get(HEADER_SECRET_TOKEN).and_then(|v| v.to_str().ok());

    if let Err(e) = signature::verify(state.webhook_secret.as_str(), token) {
        warn!(reason = %e, "Telegram webhook secret validation failed");
        return StatusCode::UNAUTHORIZED;
    }

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!(reason = %e, "Invalid JSON in Telegram webhook body");
            return StatusCode::BAD_REQUEST;
        }
    };

    let update_id = match parsed["update_id"].as_i64() {
        Some(id) => id.to_string(),
        None => {
            warn!("Telegram update missing required update_id field");
            return StatusCode::BAD_REQUEST;
        }
    };

    let update_type = extract_update_type(&parsed).unwrap_or("unroutable");

    let subject = format!("{}.{}", state.subject_prefix, update_type);

    let span = tracing::Span::current();
    span.record("update_type", update_type);
    span.record("update_id", &update_id);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, update_id.as_str());
    nats_headers.insert(NATS_HEADER_UPDATE_TYPE, update_type);
    nats_headers.insert(NATS_HEADER_UPDATE_ID, update_id.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout.into())
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests {
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

    mod ack_test_support {
        use super::*;
        use async_nats::jetstream::publish::PublishAck;
        use std::sync::Arc;
        use std::sync::Mutex;
        use trogon_nats::mocks::MockError;

        #[derive(Clone)]
        enum AckBehavior {
            Fail,
            Hang,
        }

        #[derive(Clone)]
        pub struct AckFailPublisher {
            behavior: Arc<Mutex<AckBehavior>>,
        }

        impl AckFailPublisher {
            pub fn failing() -> Self {
                Self {
                    behavior: Arc::new(Mutex::new(AckBehavior::Fail)),
                }
            }

            pub fn hanging() -> Self {
                Self {
                    behavior: Arc::new(Mutex::new(AckBehavior::Hang)),
                }
            }
        }

        pub enum AckFuture {
            Fail,
            Hang,
        }

        impl IntoFuture for AckFuture {
            type Output = Result<PublishAck, MockError>;
            type IntoFuture = std::pin::Pin<Box<dyn Future<Output = Self::Output> + Send>>;

            fn into_future(self) -> Self::IntoFuture {
                match self {
                    AckFuture::Fail => Box::pin(async { Err(MockError("simulated ack failure".to_string())) }),
                    AckFuture::Hang => Box::pin(std::future::pending()),
                }
            }
        }

        impl JetStreamPublisher for AckFailPublisher {
            type PublishError = MockError;
            type AckFuture = AckFuture;

            async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
                &self,
                _subject: S,
                _headers: async_nats::HeaderMap,
                _payload: Bytes,
            ) -> Result<AckFuture, MockError> {
                let behavior = self.behavior.lock().unwrap().clone();
                match behavior {
                    AckBehavior::Fail => Ok(AckFuture::Fail),
                    AckBehavior::Hang => Ok(AckFuture::Hang),
                }
            }
        }
    }

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
}
