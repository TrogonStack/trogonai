use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::config::LinearConfig;
use crate::signature;
#[cfg(not(coverage))]
use async_nats::jetstream::context::CreateStreamError;
use axum::{
    Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode, routing::get,
    routing::post,
};
use tracing::{info, instrument, warn};
use trogon_nats::NatsToken;
#[cfg(not(coverage))]
use trogon_nats::jetstream::JetStreamContext;
use trogon_nats::jetstream::{JetStreamPublisher, PublishOutcome, publish_event};

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

fn outcome_to_status<E: fmt::Display>(outcome: PublishOutcome<E>) -> StatusCode {
    if outcome.is_ok() {
        info!("Published Linear event to NATS");
        StatusCode::OK
    } else {
        outcome.log_on_error("linear");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Clone)]
struct AppState<P: JetStreamPublisher> {
    js: P,
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

pub fn router<P: JetStreamPublisher>(js: P, config: &LinearConfig) -> Router {
    let state = AppState {
        js,
        webhook_secret: config.webhook_secret.clone(),
        subject_prefix: config.subject_prefix.clone(),
        timestamp_tolerance: config.timestamp_tolerance,
        nats_ack_timeout: config.nats_ack_timeout,
    };

    Router::new()
        .route("/webhook", post(handle_webhook::<P>))
        .route("/health", get(handle_health))
        .with_state(state)
}

#[cfg(not(coverage))]
pub async fn serve<J>(js: J, config: LinearConfig) -> Result<(), ServeError>
where
    J: JetStreamContext<Error = CreateStreamError> + JetStreamPublisher,
{
    provision(&js, &config)
        .await
        .map_err(ServeError::Provision)?;

    let app = router(js, &config);
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

fn handle_webhook<P: JetStreamPublisher>(
    State(state): State<AppState<P>>,
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
async fn handle_webhook_inner<P: JetStreamPublisher>(
    state: AppState<P>,
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
            return StatusCode::BAD_REQUEST;
        }
    };

    if let Some(tolerance) = state.timestamp_tolerance {
        let Some(ts_ms) = parsed.get("webhookTimestamp").and_then(|v| v.as_u64()) else {
            warn!("Missing or malformed 'webhookTimestamp' field");
            return StatusCode::BAD_REQUEST;
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
            return StatusCode::BAD_REQUEST;
        }
    }

    let Some(raw_type) = parsed.get("type").and_then(|v| v.as_str()) else {
        warn!("Missing 'type' field in Linear webhook payload");
        return StatusCode::BAD_REQUEST;
    };
    let Ok(event_type) = NatsToken::new(raw_type) else {
        warn!("Invalid 'type' field in Linear webhook payload");
        return StatusCode::BAD_REQUEST;
    };

    let Some(raw_action) = parsed.get("action").and_then(|v| v.as_str()) else {
        warn!("Missing 'action' field in Linear webhook payload");
        return StatusCode::BAD_REQUEST;
    };
    let Ok(action) = NatsToken::new(raw_action) else {
        warn!("Invalid 'action' field in Linear webhook payload");
        return StatusCode::BAD_REQUEST;
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
