use std::future::IntoFuture as _;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::config::LinearConfig;
use crate::signature;
use async_nats::jetstream::context::{CreateStreamError, PublishError};
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::{self, stream};
use axum::{
    Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode, routing::get,
    routing::post,
};
use std::net::SocketAddr;
use tracing::{info, instrument, warn};

// ── JetStream operation types ─────────────────────────────────────────────────

/// Boxed future that resolves to the JetStream publish acknowledgement.
type BoxAckFut =
    Pin<Box<dyn std::future::Future<Output = Result<PublishAck, PublishError>> + Send>>;

/// Injectable publish function: `(subject, headers, payload) → Result<ack_future, error>`.
///
/// Abstracting this allows unit tests to inject fakes that simulate ACK timeouts
/// and ACK errors without requiring a live NATS server.
type PublishFn = Arc<
    dyn Fn(
            String,
            async_nats::HeaderMap,
            Bytes,
        )
            -> Pin<Box<dyn std::future::Future<Output = Result<BoxAckFut, PublishError>> + Send>>
        + Send
        + Sync,
>;

/// Injectable stream-creation function: `(stream_name, subject_prefix, max_age) → Result<(), error>`.
type EnsureStreamFn = Arc<
    dyn Fn(
            String,
            String,
            Duration,
        )
            -> Pin<Box<dyn std::future::Future<Output = Result<(), CreateStreamError>> + Send>>
        + Send
        + Sync,
>;

// ── Application state ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    publish: PublishFn,
    ensure_stream_fn: EnsureStreamFn,
    webhook_secret: Option<String>,
    subject_prefix: String,
    timestamp_tolerance: Option<Duration>,
    stream_name: String,
    stream_max_age: Duration,
    ack_timeout: Duration,
    stream_op_timeout: Duration,
}

// ── Server entry-point ────────────────────────────────────────────────────────

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

    ensure_stream(
        &js,
        &config.stream_name,
        &config.subject_prefix,
        config.stream_max_age,
    )
    .await?;

    // Build injectable closures that delegate to the real JetStream context.
    let js_pub = js.clone();
    let publish: PublishFn = Arc::new(move |subject, headers, payload| {
        let js = js_pub.clone();
        Box::pin(async move {
            let ack_future = js.publish_with_headers(subject, headers, payload).await?;
            Ok(ack_future.into_future())
        })
    });

    let js_ensure = js.clone();
    let ensure_stream_fn: EnsureStreamFn = Arc::new(move |name, prefix, max_age| {
        let js = js_ensure.clone();
        Box::pin(async move { ensure_stream(&js, &name, &prefix, max_age).await })
    });

    let state = AppState {
        publish,
        ensure_stream_fn,
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

    // ── UTF-8 multibyte sequences (bytes ≥ 128) ───────────────────────────────
    //
    // NATS subjects are byte strings and do support UTF-8 characters.
    // `has_invalid_nats_chars` only blocks the chars that NATS reserves
    // (separators, wildcards, control chars 0-31, DEL 127).
    // Bytes ≥ 128 that form valid UTF-8 code points must pass.

    #[test]
    fn latin_extended_char_passes() {
        assert!(!has_invalid_nats_chars("Issu\u{00e9}")); // é — U+00E9, two bytes
        assert!(!has_invalid_nats_chars("caf\u{00e9}")); // café
    }

    #[test]
    fn cjk_char_passes() {
        assert!(!has_invalid_nats_chars("\u{7968}")); // 票 — U+7968, three bytes
    }

    #[test]
    fn emoji_passes() {
        assert!(!has_invalid_nats_chars("\u{1f980}")); // 🦀 — U+1F980, four bytes
    }

    // ── Edge: empty string is not flagged (caught by is_empty() upstream) ─────

    #[test]
    fn empty_string_passes() {
        assert!(!has_invalid_nats_chars(""));
    }

    // ── Timeout / error path unit tests ──────────────────────────────────────
    //
    // These tests exercise the three error paths in `handle_webhook` that cannot
    // be reached reliably through a live NATS server (a local server ACKs too
    // quickly for `Duration::ZERO` to fire before the future resolves).
    //
    // The tests inject fake closures into `AppState` that return:
    //   - a future that never resolves → any finite timeout fires deterministically
    //   - an immediately-resolving error future → the retry / re-creation branch runs
    //
    // No Docker, no NATS server required.

    use super::{AppState, BoxAckFut, EnsureStreamFn, PublishFn, handle_webhook};
    use async_nats::jetstream::context::{
        CreateStreamError, CreateStreamErrorKind, PublishError, PublishErrorKind,
    };
    use axum::{body::Bytes, extract::State, http::HeaderMap, http::StatusCode};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AOrdering};
    use std::time::Duration;

    const VALID_BODY: &[u8] =
        br#"{"type":"Issue","action":"create","data":{},"webhookId":"wh-test"}"#;

    /// Build an `AppState` with injectable publish and ensure-stream closures.
    /// All other fields are set to sensible defaults for the handler under test.
    fn make_state(publish: PublishFn, ensure_stream_fn: EnsureStreamFn) -> AppState {
        AppState {
            publish,
            ensure_stream_fn,
            webhook_secret: None,
            subject_prefix: "linear".to_string(),
            timestamp_tolerance: None,
            stream_name: "LINEAR".to_string(),
            stream_max_age: Duration::from_secs(86400),
            ack_timeout: Duration::from_millis(50),
            stream_op_timeout: Duration::from_millis(50),
        }
    }

    /// A `PublishFn` whose returned ack-future resolves successfully after one
    /// yield (simulates a happy-path publish).
    fn publish_ok() -> PublishFn {
        Arc::new(|_, _, _| {
            Box::pin(async move {
                let ack_fut: BoxAckFut = Box::pin(async move {
                    // Yield once so tokio can schedule other tasks, then succeed.
                    tokio::task::yield_now().await;
                    Ok(async_nats::jetstream::publish::PublishAck {
                        stream: "LINEAR".to_string(),
                        sequence: 1,
                        duplicate: false,
                        domain: String::new(),
                        value: None,
                    })
                });
                Ok(ack_fut)
            })
        })
    }

    /// A `PublishFn` whose returned ack-future immediately returns an error
    /// (simulates a broken-pipe scenario that triggers stream re-creation).
    fn publish_ack_error() -> PublishFn {
        Arc::new(|_, _, _| {
            Box::pin(async move {
                let ack_fut: BoxAckFut =
                    Box::pin(async move { Err(PublishError::new(PublishErrorKind::BrokenPipe)) });
                Ok(ack_fut)
            })
        })
    }

    /// A `PublishFn` whose returned ack-future never resolves (simulates a
    /// NATS server that accepted the message but never sent an ACK).
    fn publish_ack_hangs() -> PublishFn {
        Arc::new(|_, _, _| {
            Box::pin(async move {
                let ack_fut: BoxAckFut = Box::pin(std::future::pending());
                Ok(ack_fut)
            })
        })
    }

    /// An `EnsureStreamFn` that resolves successfully immediately.
    fn ensure_stream_ok() -> EnsureStreamFn {
        Arc::new(|_, _, _| Box::pin(async move { Ok(()) }))
    }

    /// An `EnsureStreamFn` that immediately returns an error.
    fn ensure_stream_error() -> EnsureStreamFn {
        Arc::new(|_, _, _| {
            Box::pin(async move {
                Err(CreateStreamError::new(
                    CreateStreamErrorKind::JetStreamUnavailable,
                ))
            })
        })
    }

    /// An `EnsureStreamFn` whose future never resolves (simulates a NATS
    /// server that is unreachable for stream operations).
    fn ensure_stream_hangs() -> EnsureStreamFn {
        Arc::new(|_, _, _| Box::pin(std::future::pending()))
    }

    /// A `PublishFn` that immediately returns `Err` on every call (simulates
    /// the NATS connection being fully broken before the message is even sent).
    fn publish_fails_directly() -> PublishFn {
        Arc::new(|_, _, _| {
            Box::pin(async move {
                Err(PublishError::new(PublishErrorKind::BrokenPipe))
                    as Result<BoxAckFut, PublishError>
            })
        })
    }

    /// A stateful `PublishFn` that returns an ack error on the **first** call
    /// (triggering the recovery path) and then returns `Err` directly on the
    /// **second** call (simulating the retry publish itself failing).
    fn publish_ack_error_then_publish_fails() -> PublishFn {
        let calls = Arc::new(AtomicUsize::new(0));
        Arc::new(move |_, _, _| {
            let n = calls.fetch_add(1, AOrdering::SeqCst);
            Box::pin(async move {
                if n == 0 {
                    let ack_fut: BoxAckFut =
                        Box::pin(
                            async move { Err(PublishError::new(PublishErrorKind::BrokenPipe)) },
                        );
                    Ok(ack_fut)
                } else {
                    Err(PublishError::new(PublishErrorKind::BrokenPipe))
                        as Result<BoxAckFut, PublishError>
                }
            })
        })
    }

    /// A stateful `PublishFn` that returns an ack error on the **first** call
    /// (triggering the recovery path) and also returns an ack error on the
    /// **second** call (simulating the retry ACK failing after re-creation).
    fn publish_ack_error_both_calls() -> PublishFn {
        Arc::new(|_, _, _| {
            Box::pin(async move {
                let ack_fut: BoxAckFut =
                    Box::pin(async move { Err(PublishError::new(PublishErrorKind::BrokenPipe)) });
                Ok(ack_fut)
            })
        })
    }

    /// A stateful `PublishFn` that returns an ack error on the **first** call
    /// (triggering the recovery path) and a never-resolving ack future on the
    /// **second** call (simulating the retry ACK timing out after re-creation).
    fn publish_ack_error_then_ack_hangs() -> PublishFn {
        let calls = Arc::new(AtomicUsize::new(0));
        Arc::new(move |_, _, _| {
            let n = calls.fetch_add(1, AOrdering::SeqCst);
            Box::pin(async move {
                let ack_fut: BoxAckFut = if n == 0 {
                    Box::pin(async move { Err(PublishError::new(PublishErrorKind::BrokenPipe)) })
                } else {
                    Box::pin(std::future::pending())
                };
                Ok(ack_fut)
            })
        })
    }

    // ── 1. ACK timeout ────────────────────────────────────────────────────────

    /// When the NATS ACK future never resolves and `ack_timeout` elapses,
    /// the handler must return 500 Internal Server Error.
    #[tokio::test]
    async fn ack_timeout_returns_500() {
        let state = make_state(publish_ack_hangs(), ensure_stream_ok());

        let status = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected 500 when ACK times out"
        );
    }

    // ── 2. Stream re-creation timeout ─────────────────────────────────────────

    /// When the ACK fails and the subsequent `ensure_stream` call never resolves
    /// (e.g. NATS is unreachable), the handler must return 500.
    #[tokio::test]
    async fn stream_recreation_timeout_returns_500() {
        let state = make_state(publish_ack_error(), ensure_stream_hangs());

        let status = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected 500 when stream re-creation times out"
        );
    }

    // ── 3. Stream re-creation error ───────────────────────────────────────────

    /// When the ACK fails and `ensure_stream` returns an error (e.g. JetStream
    /// is unavailable), the handler must return 500.
    #[tokio::test]
    async fn stream_recreation_error_returns_500() {
        let state = make_state(publish_ack_error(), ensure_stream_error());

        let status = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected 500 when stream re-creation returns an error"
        );
    }

    // ── 4. Happy path (sanity check) ──────────────────────────────────────────

    /// Sanity: when publish and ACK both succeed the handler returns 200.
    #[tokio::test]
    async fn happy_path_returns_200() {
        let state = make_state(publish_ok(), ensure_stream_ok());

        let status = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "expected 200 on happy path");
    }

    // ── 5. First publish fails directly ──────────────────────────────────────
    //
    // These four tests cover the retry / post-recovery branches that cannot be
    // reached through a live NATS server:
    //
    //   • async_nats buffers aggressively — `publish_with_headers` almost never
    //     returns `Err` directly with a live connection (the error surfaces later
    //     as an ack failure), so paths 5 and 6 cannot be triggered via Docker.
    //   • Selectively delaying only the *retry* ACK (after recovery) while
    //     letting the first ACK fail quickly as an error requires a
    //     protocol-aware NATS proxy that can distinguish individual JetStream
    //     messages at the byte level — out of scope for the TCP proxy used in
    //     the e2e tests.
    //
    // Unit tests with fake closures are the correct tool here: they verify the
    // handler logic directly without depending on network timing.

    /// When the `publish_with_headers` call itself returns `Err` (e.g. the
    /// NATS client's internal acker is shut down), the handler returns 500.
    #[tokio::test]
    async fn first_publish_fails_returns_500() {
        let state = make_state(publish_fails_directly(), ensure_stream_ok());

        let status = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected 500 when first publish returns Err"
        );
    }

    // ── 6. Retry publish fails directly after stream re-creation ─────────────

    /// When the first ACK fails (triggering stream re-creation) and the retry
    /// `publish_with_headers` itself returns `Err`, the handler returns 500.
    #[tokio::test]
    async fn retry_publish_fails_returns_500() {
        let state = make_state(publish_ack_error_then_publish_fails(), ensure_stream_ok());

        let status = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected 500 when retry publish returns Err after stream re-creation"
        );
    }

    // ── 7. Retry ACK fails after stream re-creation ───────────────────────────

    /// When stream re-creation succeeds but the retry ACK returns an error,
    /// the handler returns 500.
    #[tokio::test]
    async fn retry_ack_error_returns_500() {
        let state = make_state(publish_ack_error_both_calls(), ensure_stream_ok());

        let status = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected 500 when retry ACK returns Err after stream re-creation"
        );
    }

    // ── 8. Retry ACK times out after stream re-creation ──────────────────────

    /// When stream re-creation succeeds but waiting for the retry ACK exceeds
    /// `ack_timeout`, the handler returns 500.
    #[tokio::test]
    async fn retry_ack_timeout_returns_500() {
        let state = make_state(publish_ack_error_then_ack_hangs(), ensure_stream_ok());

        let status = handle_webhook(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected 500 when retry ACK times out after stream re-creation"
        );
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
    if let Some(tolerance) = state.timestamp_tolerance
        && let Some(ts_ms) = parsed.get("webhookTimestamp").and_then(|v| v.as_u64())
    {
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

    let Some(event_type) = parsed
        .get("type")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
    else {
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

    let Some(action) = parsed
        .get("action")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
    else {
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

    match (state.publish)(subject.clone(), nats_headers, body.clone()).await {
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
                        (state.ensure_stream_fn)(
                            state.stream_name.clone(),
                            state.subject_prefix.clone(),
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
                    match (state.publish)(subject, retry_headers, body).await {
                        Ok(ack_future) => {
                            match tokio::time::timeout(state.ack_timeout, ack_future).await {
                                Ok(Ok(_)) => {
                                    info!(
                                        "Published Linear event to NATS after stream re-creation"
                                    );
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
                    warn!(
                        ack_timeout_ms = state.ack_timeout.as_millis(),
                        "NATS ack timed out"
                    );
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
