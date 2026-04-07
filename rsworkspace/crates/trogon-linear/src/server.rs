use std::future::IntoFuture as _;
use std::time::Duration;

use crate::config::LinearConfig;
use crate::signature;
use async_nats::jetstream::stream;
use axum::{
    Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode, routing::get,
    routing::post,
};
use std::net::SocketAddr;
use tracing::{info, instrument, warn};
use trogon_nats::jetstream::{JetStreamContext, JetStreamPublisher};

// ── Application state ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState<J> {
    js: J,
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
#[cfg(not(coverage))]
pub async fn serve(
    config: LinearConfig,
    nats: async_nats::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use async_nats::jetstream;
    use trogon_nats::jetstream::NatsJetStreamClient;
    serve_impl(config, NatsJetStreamClient::new(jetstream::new(nats))).await
}

async fn serve_impl<J>(
    config: LinearConfig,
    js: J,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    J: JetStreamPublisher + JetStreamContext + Clone + Send + Sync + 'static,
    <J as JetStreamContext>::Error: 'static,
{
    js.get_or_create_stream(stream::Config {
        name: config.stream_name.clone(),
        subjects: vec![format!("{}.>", config.subject_prefix)],
        max_age: config.stream_max_age,
        ..Default::default()
    })
    .await?;

    info!(
        stream = config.stream_name,
        max_age_secs = config.stream_max_age.as_secs(),
        "JetStream stream ready"
    );

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
        .route("/webhook", post(handle_webhook::<J>))
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

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

/// Returns `true` if `s` contains characters that are illegal in a NATS subject
/// token: `.` (level separator), ` ` (space), `*`/`>` (wildcards), or any
/// control character (bytes 0–31 and 127).
fn has_invalid_nats_chars(s: &str) -> bool {
    s.contains(['.', ' ', '*', '>']) || s.bytes().any(|b| b < 32 || b == 127)
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
async fn handle_webhook<J>(
    State(state): State<AppState<J>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode
where
    J: JetStreamPublisher + JetStreamContext + Clone + Send + Sync + 'static,
{
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

    match state
        .js
        .publish_with_headers(subject.clone(), nats_headers, body.clone())
        .await
    {
        Ok(ack_future) => {
            match tokio::time::timeout(state.ack_timeout, ack_future.into_future()).await {
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
                        state.js.get_or_create_stream(stream::Config {
                            name: state.stream_name.clone(),
                            subjects: vec![format!("{}.>", state.subject_prefix)],
                            max_age: state.stream_max_age,
                            ..Default::default()
                        }),
                    )
                    .await;
                    match recreate {
                        Ok(Ok(_)) => {}
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
                            match tokio::time::timeout(state.ack_timeout, ack_future.into_future())
                                .await
                            {
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::jetstream::context::{
        CreateStreamError, CreateStreamErrorKind, PublishError, PublishErrorKind,
    };
    use async_nats::jetstream::publish::PublishAck;
    use async_nats::subject::ToSubject;
    use axum::{body::Bytes, extract::State, http::HeaderMap, http::StatusCode};
    use std::future::{Future, IntoFuture};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AOrdering};
    use std::time::Duration;
    use trogon_nats::jetstream::{JetStreamContext, JetStreamPublisher};

    // ── Test JetStream client ─────────────────────────────────────────────────

    type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
    type PublishCallFn =
        Arc<dyn Fn() -> Result<BoxFut<Result<PublishAck, PublishError>>, PublishError> + Send + Sync>;
    type EnsureCallFn = Arc<dyn Fn() -> BoxFut<Result<(), CreateStreamError>> + Send + Sync>;

    /// Newtype wrapper so we can name `AckFuture` concretely as an associated type.
    struct TestAckFuture(BoxFut<Result<PublishAck, PublishError>>);

    impl IntoFuture for TestAckFuture {
        type Output = Result<PublishAck, PublishError>;
        type IntoFuture = BoxFut<Result<PublishAck, PublishError>>;
        fn into_future(self) -> Self::IntoFuture {
            self.0
        }
    }

    #[derive(Clone)]
    struct TestJs {
        publish_fn: PublishCallFn,
        ensure_fn: EnsureCallFn,
    }

    impl JetStreamPublisher for TestJs {
        type PublishError = PublishError;
        type AckFuture = TestAckFuture;

        async fn publish_with_headers<S: ToSubject + Send>(
            &self,
            _subject: S,
            _headers: async_nats::HeaderMap,
            _payload: Bytes,
        ) -> Result<TestAckFuture, PublishError> {
            (self.publish_fn)().map(TestAckFuture)
        }
    }

    impl JetStreamContext for TestJs {
        type Error = CreateStreamError;
        type Stream = ();

        async fn get_or_create_stream<S: Into<stream::Config> + Send>(
            &self,
            _config: S,
        ) -> Result<(), CreateStreamError> {
            (self.ensure_fn)().await
        }
    }

    fn make_js(
        publish: impl Fn() -> Result<BoxFut<Result<PublishAck, PublishError>>, PublishError>
            + Send
            + Sync
            + 'static,
        ensure: impl Fn() -> BoxFut<Result<(), CreateStreamError>> + Send + Sync + 'static,
    ) -> TestJs {
        TestJs {
            publish_fn: Arc::new(publish),
            ensure_fn: Arc::new(ensure),
        }
    }

    fn make_state(js: TestJs) -> AppState<TestJs> {
        AppState {
            js,
            webhook_secret: None,
            subject_prefix: "linear".to_string(),
            timestamp_tolerance: None,
            stream_name: "LINEAR".to_string(),
            stream_max_age: Duration::from_secs(86400),
            ack_timeout: Duration::from_millis(50),
            stream_op_timeout: Duration::from_millis(50),
        }
    }

    // ── Reusable future factories ─────────────────────────────────────────────

    fn ok_ack() -> BoxFut<Result<PublishAck, PublishError>> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            Ok(PublishAck {
                stream: "LINEAR".to_string(),
                sequence: 1,
                duplicate: false,
                domain: String::new(),
                value: None,
            })
        })
    }

    fn error_ack() -> BoxFut<Result<PublishAck, PublishError>> {
        Box::pin(async move { Err(PublishError::new(PublishErrorKind::BrokenPipe)) })
    }

    fn hanging_ack() -> BoxFut<Result<PublishAck, PublishError>> {
        Box::pin(std::future::pending())
    }

    fn ok_ensure() -> BoxFut<Result<(), CreateStreamError>> {
        Box::pin(async move { Ok(()) })
    }

    fn error_ensure() -> BoxFut<Result<(), CreateStreamError>> {
        Box::pin(async move {
            Err(CreateStreamError::new(
                CreateStreamErrorKind::JetStreamUnavailable,
            ))
        })
    }

    fn hanging_ensure() -> BoxFut<Result<(), CreateStreamError>> {
        Box::pin(std::future::pending())
    }

    const VALID_BODY: &[u8] =
        br#"{"type":"Issue","action":"create","data":{},"webhookId":"wh-test"}"#;

    // ── has_invalid_nats_chars ────────────────────────────────────────────────

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

    #[test]
    fn latin_extended_char_passes() {
        assert!(!has_invalid_nats_chars("Issu\u{00e9}"));
        assert!(!has_invalid_nats_chars("caf\u{00e9}"));
    }

    #[test]
    fn cjk_char_passes() {
        assert!(!has_invalid_nats_chars("\u{7968}"));
    }

    #[test]
    fn emoji_passes() {
        assert!(!has_invalid_nats_chars("\u{1f980}"));
    }

    #[test]
    fn empty_string_passes() {
        assert!(!has_invalid_nats_chars(""));
    }

    // ── Timeout / error path unit tests ──────────────────────────────────────

    /// When the NATS ACK future never resolves and `ack_timeout` elapses,
    /// the handler must return 500 Internal Server Error.
    #[tokio::test]
    async fn ack_timeout_returns_500() {
        let js = make_js(|| Ok(hanging_ack()), || ok_ensure());
        let state = make_state(js);

        let status = handle_webhook::<TestJs>(
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

    /// When the ACK fails and the subsequent `ensure_stream` call never resolves,
    /// the handler must return 500.
    #[tokio::test]
    async fn stream_recreation_timeout_returns_500() {
        let js = make_js(|| Ok(error_ack()), || hanging_ensure());
        let state = make_state(js);

        let status = handle_webhook::<TestJs>(
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

    /// When the ACK fails and `ensure_stream` returns an error,
    /// the handler must return 500.
    #[tokio::test]
    async fn stream_recreation_error_returns_500() {
        let js = make_js(|| Ok(error_ack()), || error_ensure());
        let state = make_state(js);

        let status = handle_webhook::<TestJs>(
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

    /// Sanity: when publish and ACK both succeed the handler returns 200.
    #[tokio::test]
    async fn happy_path_returns_200() {
        let js = make_js(|| Ok(ok_ack()), || ok_ensure());
        let state = make_state(js);

        let status = handle_webhook::<TestJs>(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(VALID_BODY),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "expected 200 on happy path");
    }

    /// When the `publish_with_headers` call itself returns `Err`, the handler returns 500.
    #[tokio::test]
    async fn first_publish_fails_returns_500() {
        let js = make_js(
            || Err(PublishError::new(PublishErrorKind::BrokenPipe)),
            || ok_ensure(),
        );
        let state = make_state(js);

        let status = handle_webhook::<TestJs>(
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

    /// When the first ACK fails (triggering stream re-creation) and the retry
    /// publish itself returns `Err`, the handler returns 500.
    #[tokio::test]
    async fn retry_publish_fails_returns_500() {
        let calls = Arc::new(AtomicUsize::new(0));
        let js = make_js(
            move || {
                let n = calls.fetch_add(1, AOrdering::SeqCst);
                if n == 0 {
                    Ok(error_ack())
                } else {
                    Err(PublishError::new(PublishErrorKind::BrokenPipe))
                }
            },
            || ok_ensure(),
        );
        let state = make_state(js);

        let status = handle_webhook::<TestJs>(
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

    /// When stream re-creation succeeds but the retry ACK returns an error,
    /// the handler returns 500.
    #[tokio::test]
    async fn retry_ack_error_returns_500() {
        let js = make_js(|| Ok(error_ack()), || ok_ensure());
        let state = make_state(js);

        let status = handle_webhook::<TestJs>(
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

    /// When stream re-creation succeeds but waiting for the retry ACK exceeds
    /// `ack_timeout`, the handler returns 500.
    #[tokio::test]
    async fn retry_ack_timeout_returns_500() {
        let calls = Arc::new(AtomicUsize::new(0));
        let js = make_js(
            move || {
                let n = calls.fetch_add(1, AOrdering::SeqCst);
                if n == 0 { Ok(error_ack()) } else { Ok(hanging_ack()) }
            },
            || ok_ensure(),
        );
        let state = make_state(js);

        let status = handle_webhook::<TestJs>(
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
