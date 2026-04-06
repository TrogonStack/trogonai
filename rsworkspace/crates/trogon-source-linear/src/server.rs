use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::config::LinearConfig;
use crate::constants::{HTTP_BODY_SIZE_MAX, NATS_HEADER_REJECT_REASON};
use crate::signature;
#[cfg(not(coverage))]
use async_nats::jetstream::context::CreateStreamError;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap,
    http::StatusCode, routing::get, routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
#[cfg(not(coverage))]
use trogon_nats::jetstream::JetStreamContext;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut, PublishOutcome,
};

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
            Self::Provision(e) => write!(f, "stream provisioning failed: {e}"),
            Self::Io(e) => write!(f, "IO error: {e}"),
        }
    }
}

#[cfg(not(coverage))]
impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Provision(e) => Some(e),
            Self::Io(e) => Some(e),
        }
    }
}

#[cfg(not(coverage))]
impl From<std::io::Error> for ServeError {
    fn from(e: std::io::Error) -> Self {
        ServeError::Io(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectReason {
    InvalidJson,
    MissingWebhookTimestamp,
    StaleWebhookTimestamp,
    MissingType,
    InvalidType,
    MissingAction,
    InvalidAction,
}

impl RejectReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::MissingWebhookTimestamp => "missing_webhook_timestamp",
            Self::StaleWebhookTimestamp => "stale_webhook_timestamp",
            Self::MissingType => "missing_type",
            Self::InvalidType => "invalid_type",
            Self::MissingAction => "missing_action",
            Self::InvalidAction => "invalid_action",
        }
    }
}

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Linear event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("linear");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    reason: RejectReason,
    body: Bytes,
    ack_timeout: Duration,
) -> StatusCode {
    let subject = format!("{subject_prefix}.unroutable");
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(NATS_HEADER_REJECT_REASON, reason.as_str());

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout)
        .await;
    if outcome.is_ok() {
        StatusCode::BAD_REQUEST
    } else {
        outcome.log_on_error("linear.unroutable");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut> {
    publisher: ClaimCheckPublisher<P, S>,
    webhook_secret: String,
    subject_prefix: NatsToken,
    timestamp_tolerance: Option<Duration>,
    nats_ack_timeout: Duration,
}

#[cfg(not(coverage))]
pub async fn provision<C: JetStreamContext>(js: &C, config: &LinearConfig) -> Result<(), C::Error> {
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: config.stream_name.to_string(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age,
        ..Default::default()
    })
    .await?;

    let max_age_secs = config.stream_max_age.as_secs();
    info!(
        stream = config.stream_name.as_str(),
        max_age_secs, "JetStream stream ready"
    );
    Ok(())
}

pub fn router<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &LinearConfig,
) -> Router {
    let state = AppState {
        publisher,
        webhook_secret: config.webhook_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        timestamp_tolerance: config.timestamp_tolerance,
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S>))
        .route("/health", get(handle_health))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

#[cfg(not(coverage))]
pub async fn serve<C, P, S>(
    context: C,
    publisher: ClaimCheckPublisher<P, S>,
    config: LinearConfig,
) -> Result<(), ServeError>
where
    C: JetStreamContext<Error = CreateStreamError>,
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    provision(&context, &config)
        .await
        .map_err(ServeError::Provision)?;

    let app = router(publisher, &config);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(addr = %addr, "Linear webhook server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(acp_telemetry::signal::shutdown_signal())
        .await?;

    info!("Linear webhook server shut down");
    Ok(())
}

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut>(
    State(state): State<AppState<P, S>>,
    headers: HeaderMap,
    body: Bytes,
) -> Pin<Box<dyn Future<Output = StatusCode> + Send>> {
    Box::pin(handle_webhook_inner(state, headers, body))
}

#[instrument(
    name = "linear.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        action = tracing::field::Empty,
        webhook_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut>(
    state: AppState<P, S>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let sig = headers
        .get("linear-signature")
        .and_then(|v| v.to_str().ok());

    match sig {
        Some(sig) if signature::verify(&state.webhook_secret, &body, sig) => {}
        Some(_) => {
            warn!("Invalid Linear webhook signature");
            return StatusCode::UNAUTHORIZED;
        }
        None => {
            warn!("Missing linear-signature header");
            return StatusCode::UNAUTHORIZED;
        }
    }

    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to parse Linear webhook body as JSON");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                RejectReason::InvalidJson,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    };

    if let Some(tolerance) = state.timestamp_tolerance {
        let Some(ts_ms) = parsed.get("webhookTimestamp").and_then(|v| v.as_u64()) else {
            warn!("Missing or malformed 'webhookTimestamp' field");
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                RejectReason::MissingWebhookTimestamp,
                body,
                state.nats_ack_timeout,
            )
            .await;
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let age_ms = now_ms.saturating_sub(ts_ms);
        if age_ms > tolerance.as_millis() as u64 {
            warn!(
                age_ms,
                tolerance_ms = tolerance.as_millis() as u64,
                "Stale webhookTimestamp — potential replay attack"
            );
            return publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                RejectReason::StaleWebhookTimestamp,
                body,
                state.nats_ack_timeout,
            )
            .await;
        }
    }

    let Some(raw_type) = parsed.get("type").and_then(|v| v.as_str()) else {
        warn!("Missing 'type' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::MissingType,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };
    let Ok(event_type) = NatsToken::new(raw_type) else {
        warn!("Invalid 'type' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::InvalidType,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let Some(raw_action) = parsed.get("action").and_then(|v| v.as_str()) else {
        warn!("Missing 'action' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::MissingAction,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };
    let Ok(action) = NatsToken::new(raw_action) else {
        warn!("Invalid 'action' field in Linear webhook payload");
        return publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            RejectReason::InvalidAction,
            body,
            state.nats_ack_timeout,
        )
        .await;
    };

    let webhook_id = parsed
        .get("webhookId")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_owned();

    let subject = format!("{}.{}.{}", state.subject_prefix, event_type, action);

    let span = tracing::Span::current();
    span.record("event_type", event_type.as_str());
    span.record("action", action.as_str());
    span.record("webhook_id", &webhook_id);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert("Nats-Msg-Id", webhook_id.as_str());

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body, state.nats_ack_timeout)
        .await;

    outcome_to_status(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use tower::ServiceExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamPublisher, MockObjectStore,
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
            webhook_secret: TEST_SECRET.to_string(),
            port: 0,
            subject_prefix: NatsToken::new("linear").expect("valid token"),
            stream_name: NatsToken::new("LINEAR").expect("valid token"),
            stream_max_age: Duration::from_secs(3600),
            timestamp_tolerance: Some(Duration::from_secs(60)),
            nats_ack_timeout: Duration::from_secs(10),
            nats: trogon_nats::NatsConfig::from_env(&trogon_std::env::InMemoryEnv::new()),
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

        builder
            .body(Body::from(body.to_vec()))
            .expect("valid request")
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
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|v| v.as_str()),
            Some(expected_reason),
        );
    }

    fn assert_no_publishes(publisher: &MockJetStreamPublisher) {
        assert!(
            publisher.published_messages().is_empty(),
            "expected no publishes"
        );
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

    #[cfg(not(coverage))]
    #[test]
    fn serve_error_display_and_source() {
        use async_nats::jetstream::context::{CreateStreamError, CreateStreamErrorKind};

        let io_err = ServeError::Io(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "port taken",
        ));
        assert_eq!(
            io_err.to_string(),
            "stream provisioning failed: port taken"
                .replace("stream provisioning failed", "IO error")
        );
        assert!(std::error::Error::source(&io_err).is_some());

        let prov_err =
            ServeError::Provision(CreateStreamError::new(CreateStreamErrorKind::TimedOut));
        let display = prov_err.to_string();
        assert!(display.contains("stream provisioning failed"), "{display}");
        assert!(std::error::Error::source(&prov_err).is_some());
    }

    #[cfg(not(coverage))]
    #[test]
    fn serve_error_from_io() {
        let io = std::io::Error::other("boom");
        let err: ServeError = io.into();
        assert!(matches!(err, ServeError::Io(_)));
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
}
