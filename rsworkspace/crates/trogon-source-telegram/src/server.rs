use std::fmt;
use std::time::Duration;

use crate::config::TelegramSourceConfig;
use crate::constants::{
    HEADER_SECRET_TOKEN, NATS_HEADER_UPDATE_ID, NATS_HEADER_UPDATE_TYPE, UPDATE_TYPES,
};
use crate::signature;
#[cfg(not(coverage))]
use async_nats::jetstream::context::CreateStreamError;
use axum::{
    Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode, routing::get,
    routing::post,
};
use std::future::Future;
use std::pin::Pin;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{info, instrument, warn};
use trogon_nats::jetstream::{JetStreamContext, JetStreamPublisher, PublishOutcome, publish_event};

#[cfg(not(coverage))]
#[derive(Debug)]
#[non_exhaustive]
pub enum ServeError {
    Provision(CreateStreamError),
    Io(std::io::Error),
}

#[cfg(not(coverage))]
impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServeError::Provision(e) => write!(f, "stream provisioning failed: {e}"),
            ServeError::Io(e) => write!(f, "server IO error: {e}"),
        }
    }
}

#[cfg(not(coverage))]
impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ServeError::Provision(e) => Some(e),
            ServeError::Io(e) => Some(e),
        }
    }
}

#[cfg(not(coverage))]
impl From<std::io::Error> for ServeError {
    fn from(e: std::io::Error) -> Self {
        ServeError::Io(e)
    }
}

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
struct AppState<P: JetStreamPublisher> {
    js: P,
    webhook_secret: String,
    subject_prefix: String,
    nats_ack_timeout: Duration,
}

pub async fn provision<C: JetStreamContext>(
    js: &C,
    config: &TelegramSourceConfig,
) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.clone(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age,
        ..Default::default()
    })
    .await?;

    let max_age_secs = config.stream_max_age.as_secs();
    info!(
        stream = config.stream_name,
        max_age_secs, "JetStream stream ready"
    );
    Ok(())
}

pub fn router<P: JetStreamPublisher>(js: P, config: &TelegramSourceConfig) -> Router {
    let state = AppState {
        js,
        webhook_secret: config.webhook_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P>))
        .route("/health", get(handle_health))
        .layer(RequestBodyLimitLayer::new(
            config.max_body_size.as_u64() as usize
        ))
        .with_state(state)
}

#[cfg(not(coverage))]
pub async fn serve<J>(js: J, config: TelegramSourceConfig) -> Result<(), ServeError>
where
    J: JetStreamContext<Error = CreateStreamError> + JetStreamPublisher,
{
    provision(&js, &config)
        .await
        .map_err(ServeError::Provision)?;

    let app = router(js, &config);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(addr = %addr, "Telegram webhook source listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(acp_telemetry::signal::shutdown_signal())
        .await?;

    info!("Telegram webhook source shut down");
    Ok(())
}

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

fn handle_webhook<P: JetStreamPublisher>(
    State(state): State<AppState<P>>,
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
async fn handle_webhook_inner<P: JetStreamPublisher>(
    state: AppState<P>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let token = headers
        .get(HEADER_SECRET_TOKEN)
        .and_then(|v| v.to_str().ok());

    if let Err(e) = signature::verify(&state.webhook_secret, token) {
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

    let outcome = publish_event(
        &state.js,
        subject,
        nats_headers,
        body,
        state.nats_ack_timeout,
    )
    .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use bytesize::ByteSize;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::{MockJetStreamContext, MockJetStreamPublisher};

    const TEST_SECRET: &str = "test-secret";

    fn test_config() -> TelegramSourceConfig {
        TelegramSourceConfig {
            webhook_secret: TEST_SECRET.to_string(),
            port: 0,
            subject_prefix: "telegram".to_string(),
            stream_name: "TELEGRAM".to_string(),
            stream_max_age: Duration::from_secs(3600),
            nats_ack_timeout: Duration::from_secs(10),
            max_body_size: ByteSize::mib(10),
            nats: trogon_nats::NatsConfig::from_env(&trogon_std::env::InMemoryEnv::new()),
        }
    }

    fn tracing_guard() -> tracing::subscriber::DefaultGuard {
        tracing_subscriber::fmt().with_test_writer().set_default()
    }

    fn mock_app(publisher: MockJetStreamPublisher) -> Router {
        router(publisher, &test_config())
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

    #[cfg(not(coverage))]
    #[test]
    fn serve_error_display_and_source() {
        use async_nats::jetstream::context::{CreateStreamError, CreateStreamErrorKind};

        let io_err = ServeError::Io(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "port taken",
        ));
        assert_eq!(io_err.to_string(), "server IO error: port taken");
        assert!(std::error::Error::source(&io_err).is_some());

        let prov_err = ServeError::Provision(CreateStreamError::new(
            CreateStreamErrorKind::EmptyStreamName,
        ));
        assert!(prov_err.to_string().contains("stream provisioning failed"));
        assert!(std::error::Error::source(&prov_err).is_some());

        let io_err: ServeError = std::io::Error::other("boom").into();
        assert!(matches!(io_err, ServeError::Io(_)));
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

        let resp = app
            .oneshot(webhook_request(&body, Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "telegram.message");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_UPDATE_TYPE)
                .map(|v| v.as_str()),
            Some("message"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_UPDATE_ID)
                .map(|v| v.as_str()),
            Some("12345"),
        );
    }

    #[tokio::test]
    async fn callback_query_routes_to_correct_subject() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = update_json("callback_query");

        let resp = app
            .oneshot(webhook_request(&body, Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["telegram.callback_query"]
        );
    }

    #[tokio::test]
    async fn unknown_update_type_routes_to_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"update_id": 99}"#;

        let resp = app
            .oneshot(webhook_request(body, Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(publisher.published_subjects(), vec!["telegram.unroutable"]);
    }

    #[tokio::test]
    async fn missing_update_id_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"message": {"chat": {"id": 1}}}"#;

        let resp = app
            .oneshot(webhook_request(body, Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn non_integer_update_id_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"update_id": "not_a_number", "message": {"chat": {"id": 1}}}"#;

        let resp = app
            .oneshot(webhook_request(body, Some(TEST_SECRET)))
            .await
            .unwrap();

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

        let resp = app
            .oneshot(webhook_request(b"{}", Some("wrong-secret")))
            .await
            .unwrap();

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

        let resp = app
            .oneshot(webhook_request(&body, Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn subject_uses_configured_prefix() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();

        let state = AppState {
            js: publisher.clone(),
            webhook_secret: TEST_SECRET.to_string(),
            subject_prefix: "custom".to_string(),
            nats_ack_timeout: Duration::from_secs(10),
        };

        let app = Router::new()
            .route("/webhook", post(handle_webhook::<MockJetStreamPublisher>))
            .with_state(state);

        let body = update_json("message");

        let resp = app
            .oneshot(webhook_request(&body, Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(publisher.published_subjects(), vec!["custom.message"]);
    }

    #[tokio::test]
    async fn health_endpoint_returns_200() {
        let _guard = tracing_guard();
        let app = mock_app(MockJetStreamPublisher::new());

        let req = Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_id_used_as_nats_message_id() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = update_json("message");

        let resp = app
            .oneshot(webhook_request(&body, Some(TEST_SECRET)))
            .await
            .unwrap();

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
            js: publisher.clone(),
            webhook_secret: TEST_SECRET.to_string(),
            subject_prefix: "telegram".to_string(),
            nats_ack_timeout: Duration::from_secs(10),
        };

        let app = Router::new()
            .route("/webhook", post(handle_webhook::<MockJetStreamPublisher>))
            .layer(RequestBodyLimitLayer::new(64))
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
                    AckFuture::Fail => {
                        Box::pin(async { Err(MockError("simulated ack failure".to_string())) })
                    }
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
            js: publisher,
            webhook_secret: TEST_SECRET.to_string(),
            subject_prefix: "telegram".to_string(),
            nats_ack_timeout: Duration::from_secs(10),
        };

        let app = Router::new()
            .route("/webhook", post(handle_webhook::<AckFailPublisher>))
            .with_state(state);

        let body = update_json("message");

        let resp = app
            .oneshot(webhook_request(&body, Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn ack_timeout_returns_500() {
        let _guard = tracing_guard();
        let publisher = AckFailPublisher::hanging();

        let state = AppState {
            js: publisher,
            webhook_secret: TEST_SECRET.to_string(),
            subject_prefix: "telegram".to_string(),
            nats_ack_timeout: Duration::from_millis(10),
        };

        let app = Router::new()
            .route("/webhook", post(handle_webhook::<AckFailPublisher>))
            .with_state(state);

        let body = update_json("message");

        let resp = app
            .oneshot(webhook_request(&body, Some(TEST_SECRET)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
