use std::time::Duration;

use crate::config::DatadogConfig;
use crate::signature;
use async_nats::jetstream::{self, stream};
use axum::{
    Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode, routing::get,
    routing::post,
};
use std::net::SocketAddr;
use tracing::{info, instrument, warn};

#[derive(Clone)]
struct AppState {
    js: jetstream::Context,
    webhook_secret: Option<String>,
    subject_prefix: String,
}

/// Starts the Datadog webhook HTTP server.
///
/// Ensures the JetStream stream exists (capturing `{prefix}.>`), then listens
/// for incoming requests:
/// - `POST /webhook` — receives Datadog webhook events and publishes to NATS JetStream
/// - `GET  /health`  — liveness probe, always returns 200 OK
pub async fn serve(
    config: DatadogConfig,
    nats: async_nats::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let js = jetstream::new(nats);

    ensure_stream(
        &js,
        &config.stream_name,
        &config.subject_prefix,
        config.stream_max_age,
    )
    .await?;

    let state = AppState {
        js,
        webhook_secret: config.webhook_secret,
        subject_prefix: config.subject_prefix,
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health", get(handle_health))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(addr = %addr, "Datadog webhook server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Datadog webhook server shut down");
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("failed to register SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => { info!("Received SIGTERM, shutting down"); }
        _ = sigint.recv()  => { info!("Received SIGINT, shutting down"); }
    }
}

async fn ensure_stream(
    js: &jetstream::Context,
    stream_name: &str,
    subject_prefix: &str,
    max_age: Duration,
) -> Result<(), async_nats::jetstream::context::CreateStreamError> {
    js.get_or_create_stream(stream::Config {
        name: stream_name.to_string(),
        subjects: vec![format!("{}.>", subject_prefix)],
        max_age,
        ..Default::default()
    })
    .await?;

    info!(
        stream = stream_name,
        max_age_secs = max_age.as_secs(),
        "JetStream stream ready"
    );
    Ok(())
}

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

/// Derives the NATS event type from the Datadog payload.
///
/// Datadog sends an `alert_transition` field indicating the state change:
/// - `"Triggered"` → `alert`
/// - `"Recovered"` → `alert.recovered`
/// - `"Re-Triggered"` → `alert`
/// - anything else → `event`
pub(crate) fn event_type(body: &[u8]) -> String {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return "event".to_string();
    };
    match v["alert_transition"].as_str() {
        Some("Triggered") | Some("Re-Triggered") => "alert".to_string(),
        Some("Recovered") => "alert.recovered".to_string(),
        _ => "event".to_string(),
    }
}

#[instrument(
    name = "datadog.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        request_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if let Some(secret) = &state.webhook_secret {
        let sig = headers
            .get("x-datadog-signature")
            .and_then(|v| v.to_str().ok());

        match sig {
            Some(sig) if signature::verify(secret, &body, sig) => {}
            Some(_) => {
                warn!("Invalid Datadog webhook signature");
                return StatusCode::UNAUTHORIZED;
            }
            None => {
                warn!("Missing X-Datadog-Signature header");
                return StatusCode::UNAUTHORIZED;
            }
        }
    }

    let request_id = headers
        .get("dd-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_owned();

    let ev_type = event_type(&body);
    let subject = format!("{}.{}", state.subject_prefix, ev_type);

    let span = tracing::Span::current();
    span.record("event_type", &ev_type);
    span.record("request_id", &request_id);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert("X-Datadog-Event-Type", ev_type.as_str());
    nats_headers.insert("DD-Request-ID", request_id.as_str());

    const NATS_ACK_TIMEOUT: Duration = Duration::from_secs(10);

    match state
        .js
        .publish_with_headers(subject.clone(), nats_headers, body)
        .await
    {
        Ok(ack_future) => match tokio::time::timeout(NATS_ACK_TIMEOUT, ack_future).await {
            Ok(Ok(_)) => {
                info!("Published Datadog event to NATS");
                StatusCode::OK
            }
            Ok(Err(e)) => {
                warn!(error = %e, "NATS ack failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Err(_) => {
                warn!("NATS ack timed out after {NATS_ACK_TIMEOUT:?}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
        Err(e) => {
            warn!(error = %e, "Failed to publish Datadog event to NATS");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggered_maps_to_alert() {
        assert_eq!(event_type(br#"{"alert_transition":"Triggered"}"#), "alert");
    }

    #[test]
    fn retriggered_maps_to_alert() {
        assert_eq!(
            event_type(br#"{"alert_transition":"Re-Triggered"}"#),
            "alert"
        );
    }

    #[test]
    fn recovered_maps_to_alert_recovered() {
        assert_eq!(
            event_type(br#"{"alert_transition":"Recovered"}"#),
            "alert.recovered"
        );
    }

    #[test]
    fn missing_alert_transition_maps_to_event() {
        assert_eq!(event_type(br#"{"monitor_name":"something"}"#), "event");
    }

    #[test]
    fn unknown_transition_value_maps_to_event() {
        assert_eq!(event_type(br#"{"alert_transition":"Muted"}"#), "event");
    }

    #[test]
    fn invalid_json_maps_to_event() {
        assert_eq!(event_type(b"not json at all"), "event");
    }

    #[test]
    fn empty_body_maps_to_event() {
        assert_eq!(event_type(b""), "event");
    }

    #[test]
    fn alert_transition_null_maps_to_event() {
        assert_eq!(event_type(br#"{"alert_transition":null}"#), "event");
    }
}
