use std::fmt;
use std::time::Duration;

use crate::config::DiscordConfig;
use crate::constants::{
    HEADER_SIGNATURE, HEADER_TIMESTAMP, NATS_HEADER_INTERACTION_ID,
    NATS_HEADER_INTERACTION_TYPE,
};
use crate::signature;
#[cfg(not(coverage))]
use async_nats::jetstream::context::CreateStreamError;
use axum::{
    Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode, routing::get,
    routing::post,
};
use ed25519_dalek::VerifyingKey;
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
        info!("Published Discord interaction to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("discord");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn interaction_type_name(type_id: u64) -> Option<&'static str> {
    match type_id {
        2 => Some("application_command"),
        3 => Some("message_component"),
        4 => Some("autocomplete"),
        5 => Some("modal_submit"),
        _ => None,
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher> {
    js: P,
    public_key: VerifyingKey,
    subject_prefix: String,
    nats_ack_timeout: Duration,
}

pub async fn provision<C: JetStreamContext>(
    js: &C,
    config: &DiscordConfig,
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

pub fn router<P: JetStreamPublisher>(js: P, config: &DiscordConfig) -> Router {
    let state = AppState {
        js,
        public_key: config.public_key,
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P>))
        .route("/health", get(handle_health))
        .layer(RequestBodyLimitLayer::new(
            config.max_body_size.as_u64() as usize,
        ))
        .with_state(state)
}

#[cfg(not(coverage))]
pub async fn serve<J>(js: J, config: DiscordConfig) -> Result<(), ServeError>
where
    J: JetStreamContext<Error = CreateStreamError> + JetStreamPublisher,
{
    provision(&js, &config)
        .await
        .map_err(ServeError::Provision)?;

    let app = router(js, &config);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(addr = %addr, "Discord webhook server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(acp_telemetry::signal::shutdown_signal())
        .await?;

    info!("Discord webhook server shut down");
    Ok(())
}

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

fn handle_webhook<P: JetStreamPublisher>(
    State(state): State<AppState<P>>,
    headers: HeaderMap,
    body: Bytes,
) -> Pin<Box<dyn Future<Output = (StatusCode, String)> + Send>> {
    Box::pin(handle_webhook_inner(state, headers, body))
}

#[instrument(
    name = "discord.webhook",
    skip_all,
    fields(
        interaction_type = tracing::field::Empty,
        interaction_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher>(
    state: AppState<P>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    let Some(sig) = headers.get(HEADER_SIGNATURE).and_then(|v| v.to_str().ok()) else {
        warn!("Missing X-Signature-Ed25519 header");
        return (StatusCode::UNAUTHORIZED, String::new());
    };

    let Some(timestamp) = headers.get(HEADER_TIMESTAMP).and_then(|v| v.to_str().ok()) else {
        warn!("Missing X-Signature-Timestamp header");
        return (StatusCode::UNAUTHORIZED, String::new());
    };

    if let Err(e) = signature::verify(&state.public_key, timestamp, &body, sig) {
        warn!(reason = %e, "Discord interaction signature validation failed");
        return (StatusCode::UNAUTHORIZED, String::new());
    }

    let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body) else {
        warn!("Invalid JSON in Discord interaction body");
        return (StatusCode::BAD_REQUEST, String::new());
    };

    let Some(type_id) = parsed.get("type").and_then(|v| v.as_u64()) else {
        warn!("Missing or invalid 'type' field in interaction");
        return (StatusCode::BAD_REQUEST, String::new());
    };

    if type_id == 1 {
        info!("Responding to Discord PING");
        return (StatusCode::OK, r#"{"type":1}"#.to_owned());
    }

    let Some(type_name) = interaction_type_name(type_id) else {
        warn!(type_id, "Unknown Discord interaction type");
        return (StatusCode::BAD_REQUEST, String::new());
    };

    let interaction_id = parsed
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_owned();

    let subject = format!("{}.{}", state.subject_prefix, type_name);

    let span = tracing::Span::current();
    span.record("interaction_type", type_name);
    span.record("interaction_id", &interaction_id);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(
        async_nats::header::NATS_MESSAGE_ID,
        interaction_id.as_str(),
    );
    nats_headers.insert(NATS_HEADER_INTERACTION_TYPE, type_name);
    nats_headers.insert(NATS_HEADER_INTERACTION_ID, interaction_id.as_str());

    let outcome = publish_event(
        &state.js,
        subject,
        nats_headers,
        body,
        state.nats_ack_timeout,
    )
    .await;

    let status = outcome_to_status(outcome);

    // Discord requires a response within 3 seconds. Respond with
    // DEFERRED_CHANNEL_MESSAGE_WITH_SOURCE so the downstream consumer
    // can take its time producing the real reply via the REST API.
    if status == StatusCode::OK {
        (StatusCode::OK, r#"{"type":5}"#.to_owned())
    } else {
        (status, String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use bytesize::ByteSize;
    use ed25519_dalek::{Signer, SigningKey};
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::{MockJetStreamContext, MockJetStreamPublisher};

    fn test_keypair() -> (SigningKey, VerifyingKey) {
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    fn sign(sk: &SigningKey, timestamp: &str, body: &[u8]) -> String {
        let mut message = Vec::with_capacity(timestamp.len() + body.len());
        message.extend_from_slice(timestamp.as_bytes());
        message.extend_from_slice(body);
        hex::encode(sk.sign(&message).to_bytes())
    }

    const TEST_TIMESTAMP: &str = "1234567890";

    fn test_config(vk: VerifyingKey) -> DiscordConfig {
        DiscordConfig {
            public_key: vk,
            port: 0,
            subject_prefix: "discord".to_string(),
            stream_name: "DISCORD".to_string(),
            stream_max_age: Duration::from_secs(3600),
            nats_ack_timeout: Duration::from_secs(10),
            max_body_size: ByteSize::mib(4),
            nats: trogon_nats::NatsConfig::from_env(&trogon_std::env::InMemoryEnv::new()),
        }
    }

    fn tracing_guard() -> tracing::subscriber::DefaultGuard {
        tracing_subscriber::fmt().with_test_writer().set_default()
    }

    fn mock_app(publisher: MockJetStreamPublisher, vk: VerifyingKey) -> Router {
        router(publisher, &test_config(vk))
    }

    fn webhook_request(
        body: &[u8],
        sig: Option<&str>,
        timestamp: Option<&str>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/webhook");

        if let Some(s) = sig {
            builder = builder.header(HEADER_SIGNATURE, s);
        }
        if let Some(t) = timestamp {
            builder = builder.header(HEADER_TIMESTAMP, t);
        }

        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    async fn response_body(resp: axum::http::Response<Body>) -> String {
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(body_bytes.to_vec()).unwrap()
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
        let (_, vk) = test_keypair();
        let js = MockJetStreamContext::new();
        let config = test_config(vk);

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "DISCORD");
        assert_eq!(streams[0].subjects, vec!["discord.>"]);
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn provision_propagates_error() {
        let _guard = tracing_guard();
        let (_, vk) = test_keypair();
        let js = MockJetStreamContext::new();
        js.fail_next();
        let config = test_config(vk);

        let result = provision(&js, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ping_interaction_returns_pong() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":1}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body_str = response_body(resp).await;
        assert_eq!(body_str, r#"{"type":1}"#);
        assert!(publisher.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn application_command_publishes_and_returns_deferred() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":2,"id":"interaction-1","data":{"name":"test"}}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body_str = response_body(resp).await;
        assert_eq!(body_str, r#"{"type":5}"#);

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "discord.application_command");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_INTERACTION_TYPE)
                .map(|v| v.as_str()),
            Some("application_command"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_INTERACTION_ID)
                .map(|v| v.as_str()),
            Some("interaction-1"),
        );
    }

    #[tokio::test]
    async fn message_component_publishes_to_correct_subject() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":3,"id":"comp-1","data":{"custom_id":"btn"}}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["discord.message_component"]
        );
    }

    #[tokio::test]
    async fn autocomplete_publishes_to_correct_subject() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":4,"id":"auto-1","data":{"name":"cmd"}}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(publisher.published_subjects(), vec!["discord.autocomplete"]);
    }

    #[tokio::test]
    async fn modal_submit_publishes_to_correct_subject() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":5,"id":"modal-1","data":{"custom_id":"form"}}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["discord.modal_submit"]
        );
    }

    #[tokio::test]
    async fn invalid_signature_returns_401() {
        let _guard = tracing_guard();
        let (_, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);

        let resp = app
            .oneshot(webhook_request(
                br#"{"type":2,"id":"x"}"#,
                Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
                Some(TEST_TIMESTAMP),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn missing_signature_returns_401() {
        let _guard = tracing_guard();
        let (_, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);

        let resp = app
            .oneshot(webhook_request(
                br#"{"type":2,"id":"x"}"#,
                None,
                Some(TEST_TIMESTAMP),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn missing_timestamp_returns_401() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":2,"id":"x"}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), None))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn invalid_json_returns_400() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = b"not json";
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_type_field_returns_400() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"id":"x"}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_interaction_type_returns_400() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":99,"id":"x"}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(publisher.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":2,"id":"x"}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn subject_uses_configured_prefix() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();

        let state = AppState {
            js: publisher.clone(),
            public_key: vk,
            subject_prefix: "custom".to_string(),
            nats_ack_timeout: Duration::from_secs(10),
        };

        let app = Router::new()
            .route("/webhook", post(handle_webhook::<MockJetStreamPublisher>))
            .with_state(state);

        let body = br#"{"type":2,"id":"x","data":{"name":"test"}}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["custom.application_command"]
        );
    }

    #[tokio::test]
    async fn health_endpoint_returns_200() {
        let _guard = tracing_guard();
        let (_, vk) = test_keypair();
        let app = mock_app(MockJetStreamPublisher::new(), vk);

        let req = Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_interaction_id_defaults_to_unknown() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), vk);
        let body = br#"{"type":2,"data":{"name":"test"}}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_INTERACTION_ID)
                .map(|v| v.as_str()),
            Some("unknown"),
        );
    }

    #[tokio::test]
    async fn body_exceeding_limit_returns_413() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = MockJetStreamPublisher::new();

        let state = AppState {
            js: publisher.clone(),
            public_key: vk,
            subject_prefix: "discord".to_string(),
            nats_ack_timeout: Duration::from_secs(10),
        };

        let app = Router::new()
            .route("/webhook", post(handle_webhook::<MockJetStreamPublisher>))
            .layer(RequestBodyLimitLayer::new(64))
            .with_state(state);

        let oversized_body = vec![0u8; 128];
        let sig = sign(&sk, TEST_TIMESTAMP, &oversized_body);
        let resp = app
            .oneshot(webhook_request(
                &oversized_body,
                Some(&sig),
                Some(TEST_TIMESTAMP),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(publisher.published_messages().is_empty());
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
            public_key: VerifyingKey,
        }

        impl AckFailPublisher {
            pub fn failing(vk: VerifyingKey) -> Self {
                Self {
                    behavior: Arc::new(Mutex::new(AckBehavior::Fail)),
                    public_key: vk,
                }
            }

            pub fn hanging(vk: VerifyingKey) -> Self {
                Self {
                    behavior: Arc::new(Mutex::new(AckBehavior::Hang)),
                    public_key: vk,
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

        pub fn ack_fail_app(publisher: AckFailPublisher) -> Router {
            let state = AppState {
                public_key: publisher.public_key,
                js: publisher,
                subject_prefix: "discord".to_string(),
                nats_ack_timeout: Duration::from_secs(10),
            };

            Router::new()
                .route("/webhook", post(handle_webhook::<AckFailPublisher>))
                .with_state(state)
        }

        pub fn ack_hang_app(publisher: AckFailPublisher) -> Router {
            let state = AppState {
                public_key: publisher.public_key,
                js: publisher,
                subject_prefix: "discord".to_string(),
                nats_ack_timeout: Duration::from_millis(10),
            };

            Router::new()
                .route("/webhook", post(handle_webhook::<AckFailPublisher>))
                .with_state(state)
        }
    }

    use ack_test_support::{AckFailPublisher, ack_fail_app, ack_hang_app};

    #[tokio::test]
    async fn ack_failure_returns_500() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = AckFailPublisher::failing(vk);
        let app = ack_fail_app(publisher);
        let body = br#"{"type":2,"id":"x"}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn ack_timeout_returns_500() {
        let _guard = tracing_guard();
        let (sk, vk) = test_keypair();
        let publisher = AckFailPublisher::hanging(vk);
        let app = ack_hang_app(publisher);
        let body = br#"{"type":2,"id":"x"}"#;
        let sig = sign(&sk, TEST_TIMESTAMP, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig), Some(TEST_TIMESTAMP)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
