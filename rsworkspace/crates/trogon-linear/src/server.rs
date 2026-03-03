use std::time::Duration;

use crate::config::LinearConfig;
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
    timestamp_tolerance: Option<Duration>,
    stream_name: String,
    stream_max_age: Duration,
    ack_timeout: Duration,
    stream_op_timeout: Duration,
}

/// Starts the Linear webhook HTTP server.
///
/// Ensures the JetStream stream exists (capturing `{prefix}.>`), then listens
/// for incoming requests:
/// - `POST /webhook` — receives Linear webhook events and publishes to NATS JetStream
/// - `GET  /health`  — liveness probe, always returns 200 OK
pub async fn serve(
    config: LinearConfig,
    nats: async_nats::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let js = jetstream::new(nats);

    ensure_stream(&js, &config.stream_name, &config.subject_prefix, config.stream_max_age).await?;

    let state = AppState {
        js,
        webhook_secret: config.webhook_secret,
        subject_prefix: config.subject_prefix,
        timestamp_tolerance: config.timestamp_tolerance,
        stream_name: config.stream_name,
        stream_max_age: config.stream_max_age,
        ack_timeout: config.nats_ack_timeout,
        stream_op_timeout: config.nats_stream_op_timeout,
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health", get(handle_health))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(addr = %addr, "Linear webhook server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Linear webhook server shut down");
    Ok(())
}

/// Resolves when SIGTERM or SIGINT is received, allowing in-flight requests
/// to complete before the process exits.
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

    info!(stream = stream_name, max_age_secs = max_age.as_secs(), "JetStream stream ready");
    Ok(())
}

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

/// Returns `true` if `s` contains characters that are illegal in a NATS subject
/// token: `.` (level separator), ` ` (space), `*`/`>` (wildcards), or any
/// control character (bytes 0–31 and 127).
fn has_invalid_nats_chars(s: &str) -> bool {
    s.contains(['.', ' ', '*', '>']) || s.bytes().any(|b| b < 32 || b == 127)
}

#[cfg(test)]
mod tests {
    use super::has_invalid_nats_chars;

    // ── Valid tokens ──────────────────────────────────────────────────────────

    #[test]
    fn valid_alphanumeric_passes() {
        assert!(!has_invalid_nats_chars("Issue"));
        assert!(!has_invalid_nats_chars("create"));
        assert!(!has_invalid_nats_chars("ProjectUpdate"));
    }

    #[test]
    fn valid_with_hyphen_and_underscore_passes() {
        assert!(!has_invalid_nats_chars("foo_bar"));
        assert!(!has_invalid_nats_chars("foo-bar"));
    }

    // ── Separator / wildcard chars (branch 1) ─────────────────────────────────

    #[test]
    fn dot_fails() {
        assert!(has_invalid_nats_chars("foo.bar"));
        assert!(has_invalid_nats_chars("."));
    }

    #[test]
    fn space_fails() {
        assert!(has_invalid_nats_chars("foo bar"));
        assert!(has_invalid_nats_chars(" "));
    }

    #[test]
    fn star_wildcard_fails() {
        assert!(has_invalid_nats_chars("foo*"));
        assert!(has_invalid_nats_chars("*"));
    }

    #[test]
    fn gt_wildcard_fails() {
        assert!(has_invalid_nats_chars("foo>"));
        assert!(has_invalid_nats_chars(">"));
    }

    // ── Control characters (branch 2) ─────────────────────────────────────────

    #[test]
    fn null_byte_fails() {
        assert!(has_invalid_nats_chars("foo\0bar"));
    }

    #[test]
    fn tab_fails() {
        assert!(has_invalid_nats_chars("foo\tbar"));
    }

    #[test]
    fn newline_fails() {
        assert!(has_invalid_nats_chars("foo\nbar"));
    }

    #[test]
    fn carriage_return_fails() {
        assert!(has_invalid_nats_chars("foo\rbar"));
    }

    #[test]
    fn del_byte_127_fails() {
        assert!(has_invalid_nats_chars("foo\x7fbar"));
    }

    // ── Edge: empty string is not flagged (caught by is_empty() upstream) ─────

    #[test]
    fn empty_string_passes() {
        assert!(!has_invalid_nats_chars(""));
    }
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
async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Verify signature if a secret is configured.
    if let Some(secret) = &state.webhook_secret {
        let sig = headers
            .get("linear-signature")
            .and_then(|v| v.to_str().ok());

        match sig {
            Some(sig) if signature::verify(secret, &body, sig) => {}
            Some(_) => {
                warn!("Invalid Linear webhook signature");
                return StatusCode::UNAUTHORIZED;
            }
            None => {
                warn!("Missing linear-signature header");
                return StatusCode::UNAUTHORIZED;
            }
        }
    }

    // Parse the body JSON to extract `type` and `action`.
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to parse Linear webhook body as JSON");
            return StatusCode::BAD_REQUEST;
        }
    };

    // Replay-attack protection: reject events whose `webhookTimestamp` (ms)
    // falls outside the configured tolerance window.
    if let Some(tolerance) = state.timestamp_tolerance {
        if let Some(ts_ms) = parsed.get("webhookTimestamp").and_then(|v| v.as_u64()) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let age_ms = now_ms.saturating_sub(ts_ms);
            if age_ms > tolerance.as_millis() as u64 {
                warn!(age_ms, tolerance_ms = tolerance.as_millis() as u64, "Stale webhookTimestamp — potential replay attack");
                return StatusCode::BAD_REQUEST;
            }
        }
    }

    let Some(event_type) = parsed.get("type").and_then(|v| v.as_str()).map(str::to_owned) else {
        warn!("Missing 'type' field in Linear webhook payload");
        return StatusCode::BAD_REQUEST;
    };
    if event_type.is_empty() {
        warn!("Empty 'type' field in Linear webhook payload");
        return StatusCode::BAD_REQUEST;
    }
    if has_invalid_nats_chars(&event_type) {
        warn!("Invalid NATS subject characters in 'type' field");
        return StatusCode::BAD_REQUEST;
    }

    let Some(action) = parsed.get("action").and_then(|v| v.as_str()).map(str::to_owned) else {
        warn!("Missing 'action' field in Linear webhook payload");
        return StatusCode::BAD_REQUEST;
    };
    if action.is_empty() {
        warn!("Empty 'action' field in Linear webhook payload");
        return StatusCode::BAD_REQUEST;
    }
    if has_invalid_nats_chars(&action) {
        warn!("Invalid NATS subject characters in 'action' field");
        return StatusCode::BAD_REQUEST;
    }

    let webhook_id = parsed
        .get("webhookId")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_owned();

    let subject = format!("{}.{}.{}", state.subject_prefix, event_type, action);

    let span = tracing::Span::current();
    span.record("event_type", &event_type);
    span.record("action", &action);
    span.record("webhook_id", &webhook_id);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert("X-Linear-Type", event_type.as_str());
    nats_headers.insert("X-Linear-Action", action.as_str());
    nats_headers.insert("X-Linear-Webhook-Id", webhook_id.as_str());

    match state
        .js
        .publish_with_headers(subject.clone(), nats_headers, body.clone())
        .await
    {
        Ok(ack_future) => {
            match tokio::time::timeout(state.ack_timeout, ack_future).await {
                Ok(Ok(_)) => {
                    info!("Published Linear event to NATS");
                    StatusCode::OK
                }
                Ok(Err(e)) => {
                    // Ack failed — the stream may have been lost (e.g. NATS restarted).
                    // Re-create the stream and retry the publish once.
                    warn!(error = %e, "NATS ack failed — attempting stream re-creation");
                    let recreate = tokio::time::timeout(
                        state.stream_op_timeout,
                        ensure_stream(
                            &state.js,
                            &state.stream_name,
                            &state.subject_prefix,
                            state.stream_max_age,
                        ),
                    )
                    .await;
                    match recreate {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            warn!(error = %e, "Failed to re-create JetStream stream");
                            return StatusCode::INTERNAL_SERVER_ERROR;
                        }
                        Err(_) => {
                            warn!("Timed out waiting for JetStream stream re-creation");
                            return StatusCode::INTERNAL_SERVER_ERROR;
                        }
                    }
                    let mut retry_headers = async_nats::HeaderMap::new();
                    retry_headers.insert("X-Linear-Type", event_type.as_str());
                    retry_headers.insert("X-Linear-Action", action.as_str());
                    retry_headers.insert("X-Linear-Webhook-Id", webhook_id.as_str());
                    match state
                        .js
                        .publish_with_headers(subject, retry_headers, body)
                        .await
                    {
                        Ok(ack_future) => {
                            match tokio::time::timeout(state.ack_timeout, ack_future).await {
                                Ok(Ok(_)) => {
                                    info!("Published Linear event to NATS after stream re-creation");
                                    StatusCode::OK
                                }
                                Ok(Err(e)) => {
                                    warn!(error = %e, "NATS ack failed after stream re-creation");
                                    StatusCode::INTERNAL_SERVER_ERROR
                                }
                                Err(_) => {
                                    warn!("NATS ack timed out after stream re-creation");
                                    StatusCode::INTERNAL_SERVER_ERROR
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to publish Linear event to NATS after stream re-creation");
                            StatusCode::INTERNAL_SERVER_ERROR
                        }
                    }
                }
                Err(_) => {
                    warn!(ack_timeout_ms = state.ack_timeout.as_millis(), "NATS ack timed out");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to publish Linear event to NATS");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
