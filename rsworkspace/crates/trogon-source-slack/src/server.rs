use std::fmt;
use std::time::Duration;

use crate::config::SlackConfig;
use crate::constants::{
    CONTENT_TYPE_FORM, HEADER_SIGNATURE, HEADER_TIMESTAMP, HTTP_BODY_SIZE_MAX,
    NATS_HEADER_EVENT_ID, NATS_HEADER_EVENT_TYPE, NATS_HEADER_PAYLOAD_KIND,
    NATS_HEADER_REJECT_REASON, NATS_HEADER_TEAM_ID,
};
use crate::signature;
use trogon_std::SystemClock;
use trogon_std::time::EpochClock;

#[cfg(not(coverage))]
use async_nats::jetstream::context::CreateStreamError;
use axum::{
    Router, body::Bytes, extract::DefaultBodyLimit, extract::State, http::HeaderMap,
    http::StatusCode, routing::get, routing::post,
};
use form_urlencoded;
use std::future::Future;
use std::pin::Pin;
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
        info!("Published Slack event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("slack");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn publish_unroutable<P: JetStreamPublisher, S: ObjectStorePut>(
    publisher: &ClaimCheckPublisher<P, S>,
    subject_prefix: &NatsToken,
    reason: &str,
    body: Bytes,
    ack_timeout: Duration,
) {
    let subject = format!("{}.unroutable", subject_prefix);
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(NATS_HEADER_REJECT_REASON, reason);
    headers.insert(NATS_HEADER_PAYLOAD_KIND, "unroutable");

    let outcome = publisher
        .publish_event(subject, headers, body, ack_timeout)
        .await;
    outcome.log_on_error("slack.unroutable");
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock> {
    publisher: ClaimCheckPublisher<P, S>,
    clock: C,
    signing_secret: String,
    subject_prefix: NatsToken,
    nats_ack_timeout: Duration,
    timestamp_max_drift: Duration,
}

#[cfg(not(coverage))]
pub async fn provision<C: JetStreamContext>(js: &C, config: &SlackConfig) -> Result<(), C::Error> {
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
    config: &SlackConfig,
) -> Router {
    router_with_clock(publisher, config, SystemClock)
}

fn router_with_clock<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
    publisher: ClaimCheckPublisher<P, S>,
    config: &SlackConfig,
    clock: C,
) -> Router {
    let state = AppState {
        publisher,
        clock,
        signing_secret: config.signing_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        nats_ack_timeout: config.nats_ack_timeout,
        timestamp_max_drift: config.timestamp_max_drift,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P, S, C>))
        .route("/health", get(handle_health))
        .layer(DefaultBodyLimit::max(HTTP_BODY_SIZE_MAX.as_usize()))
        .with_state(state)
}

#[cfg(not(coverage))]
pub async fn serve<C, P, S>(
    context: C,
    publisher: ClaimCheckPublisher<P, S>,
    config: SlackConfig,
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
    info!(addr = %addr, "Slack webhook server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(acp_telemetry::signal::shutdown_signal())
        .await?;

    info!("Slack webhook server shut down");
    Ok(())
}

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

fn handle_webhook<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
    State(state): State<AppState<P, S, C>>,
    headers: HeaderMap,
    body: Bytes,
) -> Pin<Box<dyn Future<Output = (StatusCode, String)> + Send>> {
    Box::pin(handle_webhook_inner(state, headers, body))
}

#[instrument(
    name = "slack.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        event_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook_inner<P: JetStreamPublisher, S: ObjectStorePut, C: EpochClock>(
    state: AppState<P, S, C>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    let Some(timestamp) = headers.get(HEADER_TIMESTAMP).and_then(|v| v.to_str().ok()) else {
        warn!("Missing X-Slack-Request-Timestamp header");
        return (StatusCode::UNAUTHORIZED, String::new());
    };

    let Ok(ts) = timestamp.parse::<u64>() else {
        warn!("Invalid X-Slack-Request-Timestamp header");
        return (StatusCode::UNAUTHORIZED, String::new());
    };

    let now = state
        .clock
        .system_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let drift = now.abs_diff(ts);
    if drift > state.timestamp_max_drift.as_secs() {
        warn!(drift_secs = drift, "Slack request timestamp too old");
        return (StatusCode::UNAUTHORIZED, String::new());
    }

    let Some(sig) = headers.get(HEADER_SIGNATURE).and_then(|v| v.to_str().ok()) else {
        warn!("Missing X-Slack-Signature header");
        return (StatusCode::UNAUTHORIZED, String::new());
    };

    if let Err(e) = signature::verify(&state.signing_secret, timestamp, &body, sig) {
        warn!(reason = %e, "Slack signature validation failed");
        return (StatusCode::UNAUTHORIZED, String::new());
    }

    let is_form = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with(CONTENT_TYPE_FORM));

    if is_form {
        handle_form_payload(&state, &body).await
    } else {
        handle_json_payload(&state, &body).await
    }
}

async fn handle_json_payload<P: JetStreamPublisher, S: ObjectStorePut>(
    state: &AppState<P, S, impl EpochClock>,
    body: &Bytes,
) -> (StatusCode, String) {
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(body) else {
        warn!("Invalid JSON payload");
        publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            "invalid_json",
            body.clone(),
            state.nats_ack_timeout,
        )
        .await;
        return (StatusCode::BAD_REQUEST, String::new());
    };

    let payload_type = payload
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    if payload_type == "url_verification" {
        let challenge = payload
            .get("challenge")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        info!("Responding to Slack URL verification challenge");
        return (StatusCode::OK, challenge);
    }

    if payload_type != "event_callback" {
        warn!(payload_type, "Unhandled payload type");
        publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            "unhandled_payload_type",
            body.clone(),
            state.nats_ack_timeout,
        )
        .await;
        return (StatusCode::OK, String::new());
    }

    let Some(event_type) = payload
        .get("event")
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
    else {
        warn!("Missing event.type in event_callback payload");
        publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            "missing_event_type",
            body.clone(),
            state.nats_ack_timeout,
        )
        .await;
        return (StatusCode::BAD_REQUEST, String::new());
    };
    let event_type = event_type.to_owned();

    let Some(event_id) = payload.get("event_id").and_then(|v| v.as_str()) else {
        warn!("Missing event_id in event_callback payload");
        publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            "missing_event_id",
            body.clone(),
            state.nats_ack_timeout,
        )
        .await;
        return (StatusCode::BAD_REQUEST, String::new());
    };
    let event_id = event_id.to_owned();

    let team_id = payload
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_owned();

    // TODO: File attachments (e.g. `message` events with `files` array) contain
    // private URLs that require a bot token to download. A downstream NATS consumer
    // should handle fetching and storing file content — the source stays a dumb pipe.
    let subject = format!("{}.event.{}", state.subject_prefix, event_type);

    let span = tracing::Span::current();
    span.record("event_type", &event_type);
    span.record("event_id", &event_id);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, event_id.as_str());
    nats_headers.insert(NATS_HEADER_EVENT_TYPE, event_type.as_str());
    nats_headers.insert(NATS_HEADER_EVENT_ID, event_id.as_str());
    nats_headers.insert(NATS_HEADER_TEAM_ID, team_id.as_str());
    nats_headers.insert(NATS_HEADER_PAYLOAD_KIND, "event");

    let outcome = state
        .publisher
        .publish_event(subject, nats_headers, body.clone(), state.nats_ack_timeout)
        .await;

    (outcome_to_status(outcome), String::new())
}

async fn handle_form_payload<P: JetStreamPublisher, S: ObjectStorePut>(
    state: &AppState<P, S, impl EpochClock>,
    body: &Bytes,
) -> (StatusCode, String) {
    let form_str = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => {
            warn!("Invalid UTF-8 in form payload");
            publish_unroutable(
                &state.publisher,
                &state.subject_prefix,
                "invalid_utf8_form",
                body.clone(),
                state.nats_ack_timeout,
            )
            .await;
            return (StatusCode::BAD_REQUEST, String::new());
        }
    };

    let fields: Vec<(String, String)> = form_urlencoded::parse(form_str.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let find_field = |name: &str| {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    if let Some(payload_json) = find_field("payload") {
        return handle_interaction(state, payload_json, body).await;
    }

    if let Some(command) = find_field("command") {
        return handle_slash_command(state, command, &fields, body).await;
    }

    warn!("Unrecognized form payload");
    publish_unroutable(
        &state.publisher,
        &state.subject_prefix,
        "unrecognized_form",
        body.clone(),
        state.nats_ack_timeout,
    )
    .await;
    (StatusCode::BAD_REQUEST, String::new())
}

async fn handle_interaction<P: JetStreamPublisher, S: ObjectStorePut>(
    state: &AppState<P, S, impl EpochClock>,
    payload_json: &str,
    raw_body: &Bytes,
) -> (StatusCode, String) {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload_json) else {
        warn!("Invalid JSON in interaction payload field");
        publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            "invalid_interaction_json",
            raw_body.clone(),
            state.nats_ack_timeout,
        )
        .await;
        return (StatusCode::BAD_REQUEST, String::new());
    };

    let Some(interaction_type) = payload.get("type").and_then(|v| v.as_str()) else {
        warn!("Missing type in interaction payload");
        publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            "missing_interaction_type",
            raw_body.clone(),
            state.nats_ack_timeout,
        )
        .await;
        return (StatusCode::BAD_REQUEST, String::new());
    };
    let interaction_type = interaction_type.to_owned();

    let Some(trigger_id) = payload.get("trigger_id").and_then(|v| v.as_str()) else {
        warn!("Missing trigger_id in interaction payload");
        publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            "missing_interaction_trigger_id",
            raw_body.clone(),
            state.nats_ack_timeout,
        )
        .await;
        return (StatusCode::BAD_REQUEST, String::new());
    };
    let trigger_id = trigger_id.to_owned();

    let team_id = payload
        .get("team")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_owned();

    let subject = format!("{}.interaction.{}", state.subject_prefix, interaction_type);

    let span = tracing::Span::current();
    span.record("event_type", &interaction_type);
    span.record("event_id", &trigger_id);
    span.record("subject", &subject);

    info!(interaction_type, "Received Slack interaction");

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, trigger_id.as_str());
    nats_headers.insert(NATS_HEADER_EVENT_TYPE, interaction_type.as_str());
    nats_headers.insert(NATS_HEADER_TEAM_ID, team_id.as_str());
    nats_headers.insert(NATS_HEADER_PAYLOAD_KIND, "interaction");

    let outcome = state
        .publisher
        .publish_event(
            subject,
            nats_headers,
            Bytes::from(payload_json.to_owned()),
            state.nats_ack_timeout,
        )
        .await;

    (outcome_to_status(outcome), String::new())
}

async fn handle_slash_command<P: JetStreamPublisher, S: ObjectStorePut>(
    state: &AppState<P, S, impl EpochClock>,
    command: &str,
    fields: &[(String, String)],
    raw_body: &Bytes,
) -> (StatusCode, String) {
    let command_name = command.trim_start_matches('/');

    let team_id = fields
        .iter()
        .find(|(k, _)| k == "team_id")
        .map(|(_, v)| v.as_str())
        .unwrap_or("unknown");

    let Some(trigger_id) = fields
        .iter()
        .find(|(k, _)| k == "trigger_id")
        .map(|(_, v)| v.as_str())
    else {
        warn!(command, "Missing trigger_id in slash command payload");
        publish_unroutable(
            &state.publisher,
            &state.subject_prefix,
            "missing_command_trigger_id",
            raw_body.clone(),
            state.nats_ack_timeout,
        )
        .await;
        return (StatusCode::BAD_REQUEST, String::new());
    };

    let subject = format!("{}.command.{}", state.subject_prefix, command_name);

    let span = tracing::Span::current();
    span.record("event_type", command);
    span.record("event_id", trigger_id);
    span.record("subject", &subject);

    info!(command, "Received Slack slash command");

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert(async_nats::header::NATS_MESSAGE_ID, trigger_id);
    nats_headers.insert(NATS_HEADER_EVENT_TYPE, command);
    nats_headers.insert(NATS_HEADER_TEAM_ID, team_id);
    nats_headers.insert(NATS_HEADER_PAYLOAD_KIND, "command");

    let outcome = state
        .publisher
        .publish_event(
            subject,
            nats_headers,
            raw_body.clone(),
            state.nats_ack_timeout,
        )
        .await;

    (outcome_to_status(outcome), String::new())
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
    #[cfg(not(coverage))]
    use trogon_nats::jetstream::MockJetStreamContext;
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

    const TEST_NOW: u64 = 1_700_000_000;

    use trogon_std::time::FixedEpochClock;

    fn compute_sig(secret: &str, timestamp: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"v0:");
        mac.update(timestamp.as_bytes());
        mac.update(b":");
        mac.update(body);
        format!("v0={}", hex::encode(mac.finalize().into_bytes()))
    }

    const TEST_SECRET: &str = "test-secret";

    fn current_timestamp() -> String {
        TEST_NOW.to_string()
    }

    fn test_config() -> SlackConfig {
        SlackConfig {
            signing_secret: TEST_SECRET.to_string(),
            port: 0,
            subject_prefix: NatsToken::new("slack").unwrap(),
            stream_name: NatsToken::new("SLACK").unwrap(),
            stream_max_age: Duration::from_secs(3600),
            nats_ack_timeout: Duration::from_secs(10),
            timestamp_max_drift: Duration::from_secs(300),
            nats: trogon_nats::NatsConfig::from_env(&trogon_std::env::InMemoryEnv::new()),
        }
    }

    fn tracing_guard() -> tracing::subscriber::DefaultGuard {
        tracing_subscriber::fmt().with_test_writer().set_default()
    }

    fn mock_app(publisher: MockJetStreamPublisher) -> Router {
        router_with_clock(
            wrap_publisher(publisher),
            &test_config(),
            FixedEpochClock::from_secs(TEST_NOW),
        )
    }

    fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str) {
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1, "expected exactly one unroutable publish");
        assert_eq!(messages[0].subject, "slack.unroutable");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_REJECT_REASON)
                .map(|v| v.as_str()),
            Some(expected_reason),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|v| v.as_str()),
            Some("unroutable"),
        );
    }

    fn event_callback_body(event_type: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": {
                "type": event_type,
                "text": "hello"
            }
        }))
        .unwrap()
    }

    fn webhook_request(body: &[u8], timestamp: &str, sig: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_TIMESTAMP, timestamp);

        if let Some(s) = sig {
            builder = builder.header(HEADER_SIGNATURE, s);
        }

        builder.body(Body::from(body.to_vec())).unwrap()
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

    #[cfg(not(coverage))]
    #[tokio::test]
    async fn provision_creates_stream() {
        let _guard = tracing_guard();
        let js = MockJetStreamContext::new();
        let config = test_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "SLACK");
        assert_eq!(streams[0].subjects, vec!["slack.>"]);
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[cfg(not(coverage))]
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
    async fn event_callback_publishes_to_nats_and_returns_200() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = event_callback_body("message");
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "slack.event.message");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT_TYPE)
                .map(|v| v.as_str()),
            Some("message"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT_ID)
                .map(|v| v.as_str()),
            Some("Ev01ABC123"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_TEAM_ID)
                .map(|v| v.as_str()),
            Some("T01ABC"),
        );
    }

    #[tokio::test]
    async fn url_verification_returns_challenge() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "url_verification",
            "challenge": "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P"
        }))
        .unwrap();
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let resp_body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&resp_body).unwrap(),
            "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P"
        );

        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn invalid_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = event_callback_body("message");
        let ts = current_timestamp();

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some("v0=deadbeef")))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_signature_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = event_callback_body("message");
        let ts = current_timestamp();

        let resp = app
            .oneshot(webhook_request(&body, &ts, None))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn missing_timestamp_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = event_callback_body("message");
        let sig = compute_sig(TEST_SECRET, "1234567890", &body);

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_SIGNATURE, sig)
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn non_numeric_timestamp_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = event_callback_body("message");
        let sig = compute_sig(TEST_SECRET, "not-a-number", &body);

        let resp = app
            .oneshot(webhook_request(&body, "not-a-number", Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn stale_timestamp_returns_401() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = event_callback_body("message");
        let stale_ts = "1000000000";
        let sig = compute_sig(TEST_SECRET, stale_ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, stale_ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn invalid_json_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = b"not json at all";
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, body);

        let resp = app
            .oneshot(webhook_request(body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_json");
    }

    #[tokio::test]
    async fn unhandled_payload_type_returns_200() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "app_rate_limited"
        }))
        .unwrap();
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_unroutable(&publisher, "unhandled_payload_type");
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = event_callback_body("message");
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn subject_uses_configured_prefix() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();

        let state = AppState {
            publisher: wrap_publisher(publisher.clone()),
            clock: FixedEpochClock::from_secs(TEST_NOW),
            signing_secret: TEST_SECRET.to_string(),
            subject_prefix: NatsToken::new("custom").unwrap(),
            nats_ack_timeout: Duration::from_secs(10),
            timestamp_max_drift: Duration::from_secs(300),
        };

        let app = Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<MockJetStreamPublisher, MockObjectStore, FixedEpochClock>),
            )
            .with_state(state);

        let body = event_callback_body("app_mention");
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["custom.event.app_mention"]
        );
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
    async fn missing_event_type_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": {}
        }))
        .unwrap();
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_event_type");
    }

    #[tokio::test]
    async fn missing_event_id_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "event_callback",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        }))
        .unwrap();
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_event_id");
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

            async fn publish_with_headers<S: ToSubject + Send>(
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
    use async_nats::subject::ToSubject;

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
            clock: FixedEpochClock::from_secs(TEST_NOW),
            signing_secret: TEST_SECRET.to_string(),
            subject_prefix: NatsToken::new("slack").unwrap(),
            nats_ack_timeout: Duration::from_secs(10),
            timestamp_max_drift: Duration::from_secs(300),
        };

        let app = Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<AckFailPublisher, MockObjectStore, FixedEpochClock>),
            )
            .with_state(state);

        let body = event_callback_body("message");
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

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
            clock: FixedEpochClock::from_secs(TEST_NOW),
            signing_secret: TEST_SECRET.to_string(),
            subject_prefix: NatsToken::new("slack").unwrap(),
            nats_ack_timeout: Duration::from_millis(10),
            timestamp_max_drift: Duration::from_secs(300),
        };

        let app = Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<AckFailPublisher, MockObjectStore, FixedEpochClock>),
            )
            .with_state(state);

        let body = event_callback_body("message");
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, &body);

        let resp = app
            .oneshot(webhook_request(&body, &ts, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    fn form_request(body: &[u8], timestamp: &str, sig: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/webhook")
            .header(HEADER_TIMESTAMP, timestamp)
            .header(HEADER_SIGNATURE, sig)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    #[tokio::test]
    async fn interaction_block_actions_publishes_to_nats() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let interaction_payload = serde_json::json!({
            "type": "block_actions",
            "trigger_id": "trigger123",
            "team": { "id": "T01ABC" },
            "user": { "id": "U01ABC" },
            "actions": [{ "action_id": "btn_click", "type": "button" }]
        });
        let form_body = format!(
            "payload={}",
            form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes())
                .collect::<String>()
        );
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "slack.interaction.block_actions");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|v| v.as_str()),
            Some("interaction"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_TEAM_ID)
                .map(|v| v.as_str()),
            Some("T01ABC"),
        );
    }

    #[tokio::test]
    async fn interaction_view_submission_publishes_to_nats() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let interaction_payload = serde_json::json!({
            "type": "view_submission",
            "trigger_id": "trigger456",
            "team": { "id": "T01ABC" },
            "user": { "id": "U01ABC" },
            "view": { "callback_id": "my_modal" }
        });
        let form_body = format!(
            "payload={}",
            form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes())
                .collect::<String>()
        );
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["slack.interaction.view_submission"]
        );
    }

    #[tokio::test]
    async fn interaction_missing_trigger_id_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let interaction_payload = serde_json::json!({
            "type": "view_closed",
            "team": { "id": "T01ABC" },
            "user": { "id": "U01ABC" }
        });
        let form_body = format!(
            "payload={}",
            form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes())
                .collect::<String>()
        );
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_interaction_trigger_id");
    }

    #[tokio::test]
    async fn slash_command_publishes_to_nats() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let form_body = "command=%2Ftrogon&text=hello+world&team_id=T01ABC&trigger_id=cmd789&user_id=U01ABC&channel_id=C01ABC";
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "slack.command.trogon");
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_PAYLOAD_KIND)
                .map(|v| v.as_str()),
            Some("command"),
        );
        assert_eq!(
            messages[0]
                .headers
                .get(NATS_HEADER_EVENT_TYPE)
                .map(|v| v.as_str()),
            Some("/trogon"),
        );
    }

    #[tokio::test]
    async fn slash_command_missing_trigger_id_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let form_body =
            "command=%2Ftrogon&text=hello&team_id=T01ABC&user_id=U01ABC&channel_id=C01ABC";
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_command_trigger_id");
    }

    #[tokio::test]
    async fn form_payload_with_invalid_json_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let form_body = "payload=not-valid-json";
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_interaction_json");
    }

    #[tokio::test]
    async fn invalid_utf8_form_payload_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let body: &[u8] = &[0xff, 0xfe, 0xfd];
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, body);

        let resp = app.oneshot(form_request(body, &ts, &sig)).await.unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "invalid_utf8_form");
    }

    #[tokio::test]
    async fn unrecognized_form_payload_returns_400() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let form_body = "foo=bar&baz=qux";
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "unrecognized_form");
    }

    #[tokio::test]
    async fn interaction_missing_type_publishes_unroutable() {
        let _guard = tracing_guard();
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());

        let interaction_payload = serde_json::json!({
            "trigger_id": "trigger123",
            "team": { "id": "T01ABC" }
        });
        let form_body = format!(
            "payload={}",
            form_urlencoded::byte_serialize(interaction_payload.to_string().as_bytes())
                .collect::<String>()
        );
        let ts = current_timestamp();
        let sig = compute_sig(TEST_SECRET, &ts, form_body.as_bytes());

        let resp = app
            .oneshot(form_request(form_body.as_bytes(), &ts, &sig))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_unroutable(&publisher, "missing_interaction_type");
    }

    #[tokio::test]
    async fn router_with_system_clock_responds_to_health() {
        let publisher = MockJetStreamPublisher::new();
        let app = router(wrap_publisher(publisher), &test_config());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }
}
