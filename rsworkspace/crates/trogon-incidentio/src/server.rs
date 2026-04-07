//! incident.io webhook receiver.
//!
//! Receives `POST /webhook` from incident.io, validates the HMAC-SHA256
//! signature, publishes the raw JSON payload to NATS JetStream, and updates
//! the `INCIDENTS` KV bucket with the latest incident state.
//!
//! # NATS message format
//!
//! - **Subject**: `{prefix}.{event_type}` (e.g. `incidentio.incident.created`)
//! - **Headers**: `X-Incident-Event-Type`, `X-Incident-Delivery`
//! - **Payload**: raw JSON body from incident.io

use std::net::SocketAddr;
use std::time::Duration;

use async_nats::jetstream::{self, stream};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use tracing::{info, instrument, warn};

use crate::config::IncidentioConfig;
use crate::events;
use crate::signature;
use crate::store::IncidentStore;

#[derive(Clone)]
struct AppState {
    js: jetstream::Context,
    incident_store: IncidentStore,
    webhook_secret: Option<String>,
    subject_prefix: String,
}

/// Start the incident.io webhook HTTP server.
///
/// Ensures JetStream stream and KV bucket exist, then listens for:
/// - `POST /webhook` — incident.io events → NATS JetStream + KV update
/// - `GET  /health`  — liveness probe
pub async fn serve(
    config: IncidentioConfig,
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

    let incident_store = IncidentStore::open(&js)
        .await
        .map_err(|e| format!("Failed to open IncidentStore: {e}"))?;

    let state = AppState {
        js,
        incident_store,
        webhook_secret: config.webhook_secret,
        subject_prefix: config.subject_prefix,
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health", get(handle_health))
        .route("/incidents", get(list_incidents))
        .route("/incidents/:id", get(get_incident_by_id))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(addr = %addr, "incident.io webhook server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("incident.io webhook server shut down");
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

#[instrument(
    name = "incidentio.webhook",
    skip_all,
    fields(
        event_type = tracing::field::Empty,
        incident_id = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Validate HMAC signature when a secret is configured.
    if let Some(secret) = &state.webhook_secret {
        let sig = headers
            .get("x-incident-signature")
            .and_then(|v| v.to_str().ok());

        match sig {
            Some(sig) if signature::verify(secret, &body, sig) => {}
            Some(_) => {
                warn!("Invalid incident.io webhook signature");
                return StatusCode::UNAUTHORIZED;
            }
            None => {
                warn!("Missing X-Incident-Signature header");
                return StatusCode::UNAUTHORIZED;
            }
        }
    }

    let delivery_id = headers
        .get("x-incident-delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_owned();

    let ev_type = events::nats_subject_suffix(&body);
    let subject = format!("{}.{}", state.subject_prefix, ev_type);

    let span = tracing::Span::current();
    span.record("event_type", &ev_type);
    span.record("subject", &subject);

    // Update the INCIDENTS KV bucket with the latest incident state.
    if let Some(inc_id) = events::incident_id(&body) {
        span.record("incident_id", &inc_id);
        if let Err(e) = state.incident_store.upsert(&inc_id, &body).await {
            warn!(incident_id = %inc_id, error = %e, "Failed to update INCIDENTS KV");
        }
    }

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert("X-Incident-Event-Type", ev_type.as_str());
    nats_headers.insert("X-Incident-Delivery", delivery_id.as_str());

    const NATS_ACK_TIMEOUT: Duration = Duration::from_secs(10);

    match state
        .js
        .publish_with_headers(subject.clone(), nats_headers, body)
        .await
    {
        Ok(ack_future) => match tokio::time::timeout(NATS_ACK_TIMEOUT, ack_future).await {
            Ok(Ok(_)) => {
                info!(subject, "Published incident.io event to NATS");
                StatusCode::OK
            }
            Ok(Err(e)) => {
                warn!(error = %e, "NATS ack failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
            Err(_) => {
                warn!("NATS ack timed out");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
        Err(e) => {
            warn!(error = %e, "Failed to publish incident.io event to NATS");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// `GET /incidents` — returns all stored incidents as a JSON array.
async fn list_incidents(
    State(state): State<AppState>,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    let raw_list = state.incident_store.list().await.map_err(|e| {
        warn!(error = %e, "Failed to list incidents from KV");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let incidents: Vec<serde_json::Value> = raw_list
        .into_iter()
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();

    Ok(Json(incidents))
}

/// `GET /incidents/:id` — returns the stored state of a single incident.
async fn get_incident_by_id(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.incident_store.get(&id).await {
        Ok(Some(bytes)) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
                warn!(incident_id = %id, error = %e, "Failed to deserialize incident JSON");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            Ok(Json(value))
        }
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            warn!(incident_id = %id, error = %e, "Failed to get incident from KV");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::events::nats_subject_suffix;

    #[test]
    fn incident_created_subject_suffix() {
        let body = br#"{"event_type":"incident.created","incident":{"id":"inc-1"}}"#;
        assert_eq!(nats_subject_suffix(body), "incident.created");
    }

    #[test]
    fn incident_resolved_subject_suffix() {
        let body = br#"{"event_type":"incident.resolved","incident":{"id":"inc-1"}}"#;
        assert_eq!(nats_subject_suffix(body), "incident.resolved");
    }
}
