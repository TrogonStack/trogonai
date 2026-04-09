use std::future::IntoFuture as _;
use std::time::Duration;

use crate::config::DatadogConfig;
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
    ack_timeout: Duration,
}

// ── Server entry-point ────────────────────────────────────────────────────────

/// Starts the Datadog webhook HTTP server.
///
/// Ensures the JetStream stream exists (capturing `{prefix}.>`), then listens
/// for incoming requests:
/// - `POST /webhook` — receives Datadog webhook events and publishes to NATS JetStream
/// - `GET  /health`  — liveness probe, always returns 200 OK
#[cfg(not(coverage))]
pub async fn serve(
    config: DatadogConfig,
    nats: async_nats::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use async_nats::jetstream;
    use trogon_nats::jetstream::NatsJetStreamClient;
    serve_impl(config, NatsJetStreamClient::new(jetstream::new(nats))).await
}

async fn serve_impl<J>(
    config: DatadogConfig,
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
        ack_timeout: Duration::from_secs(10),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<J>))
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
async fn handle_webhook<J>(
    State(state): State<AppState<J>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode
where
    J: JetStreamPublisher + Clone + Send + Sync + 'static,
{
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

    match state
        .js
        .publish_with_headers(subject.clone(), nats_headers, body)
        .await
    {
        Ok(ack_future) => {
            match tokio::time::timeout(state.ack_timeout, ack_future.into_future()).await {
                Ok(Ok(_)) => {
                    info!("Published Datadog event to NATS");
                    StatusCode::OK
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "NATS ack failed");
                    StatusCode::INTERNAL_SERVER_ERROR
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

    // ── handler tests with mocks ──────────────────────────────────────────────

    use axum::body::Body;
    use axum::http::Request;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use tower::ServiceExt as _;
    use trogon_nats::jetstream::{MockJetStreamContext, MockJetStreamPublisher};

    type HmacSha256 = Hmac<Sha256>;

    const TEST_SECRET: &str = "dd-test-secret";

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn test_config() -> DatadogConfig {
        DatadogConfig {
            webhook_secret: Some(TEST_SECRET.to_string()),
            port: 0,
            subject_prefix: "datadog".to_string(),
            stream_name: "DATADOG".to_string(),
            stream_max_age: Duration::from_secs(3600),
            nats: trogon_nats::NatsConfig::from_env(&trogon_std::env::InMemoryEnv::new()),
        }
    }

    fn mock_app(publisher: MockJetStreamPublisher) -> Router {
        let config = test_config();
        let state = AppState {
            js: publisher,
            webhook_secret: config.webhook_secret,
            subject_prefix: config.subject_prefix,
            ack_timeout: Duration::from_secs(10),
        };
        Router::new()
            .route("/webhook", post(handle_webhook::<MockJetStreamPublisher>))
            .route("/health", get(handle_health))
            .with_state(state)
    }

    fn webhook_request(body: &[u8], sig: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("dd-request-id", "req-1");
        if let Some(s) = sig {
            builder = builder.header("x-datadog-signature", s);
        }
        builder.body(Body::from(body.to_vec())).unwrap()
    }

    #[tokio::test]
    async fn health_returns_200() {
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
    async fn valid_webhook_publishes_and_returns_200() {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"alert_transition":"Triggered","monitor_name":"cpu"}"#;
        let sig = compute_sig(TEST_SECRET, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let msgs = publisher.published_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].subject, "datadog.alert");
        assert_eq!(
            msgs[0]
                .headers
                .get("X-Datadog-Event-Type")
                .map(|v| v.as_str()),
            Some("alert")
        );
    }

    #[tokio::test]
    async fn recovered_alert_uses_alert_recovered_subject() {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let body = br#"{"alert_transition":"Recovered"}"#;
        let sig = compute_sig(TEST_SECRET, body);
        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["datadog.alert.recovered"]
        );
    }

    #[tokio::test]
    async fn invalid_signature_returns_401_and_does_not_publish() {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let resp = app
            .oneshot(webhook_request(b"{}", Some("deadbeef")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn missing_signature_returns_401() {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone());
        let resp = app.oneshot(webhook_request(b"{}", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn no_secret_accepts_request_without_signature() {
        let publisher = MockJetStreamPublisher::new();
        let state = AppState {
            js: publisher.clone(),
            webhook_secret: None,
            subject_prefix: "datadog".to_string(),
            ack_timeout: Duration::from_secs(10),
        };
        let app = Router::new()
            .route("/webhook", post(handle_webhook::<MockJetStreamPublisher>))
            .with_state(state);
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(Body::from(b"{}".as_ref()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(publisher.published_subjects(), vec!["datadog.event"]);
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone());
        let body = b"{}";
        let sig = compute_sig(TEST_SECRET, body);
        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    // ── ack error / timeout ───────────────────────────────────────────────────

    mod ack_tests {
        use super::*;
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::{Arc, Mutex};
        use trogon_nats::mocks::MockError;

        #[derive(Clone)]
        enum AckBehavior {
            Fail,
            Hang,
        }

        #[derive(Clone)]
        struct AckFailPublisher {
            behavior: Arc<Mutex<AckBehavior>>,
        }

        impl AckFailPublisher {
            fn failing() -> Self {
                Self {
                    behavior: Arc::new(Mutex::new(AckBehavior::Fail)),
                }
            }
            fn hanging() -> Self {
                Self {
                    behavior: Arc::new(Mutex::new(AckBehavior::Hang)),
                }
            }
        }

        enum AckFuture {
            Fail,
            Hang,
        }

        impl IntoFuture for AckFuture {
            type Output = Result<async_nats::jetstream::publish::PublishAck, MockError>;
            type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;
            fn into_future(self) -> Self::IntoFuture {
                match self {
                    AckFuture::Fail => Box::pin(async { Err(MockError("ack error".to_string())) }),
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
                _payload: bytes::Bytes,
            ) -> Result<AckFuture, MockError> {
                Ok(match *self.behavior.lock().unwrap() {
                    AckBehavior::Fail => AckFuture::Fail,
                    AckBehavior::Hang => AckFuture::Hang,
                })
            }
        }

        fn ack_app(publisher: AckFailPublisher) -> Router {
            let state = AppState {
                js: publisher,
                webhook_secret: None,
                subject_prefix: "datadog".to_string(),
                ack_timeout: Duration::from_millis(50),
            };
            Router::new()
                .route("/webhook", post(handle_webhook::<AckFailPublisher>))
                .with_state(state)
        }

        fn plain_request() -> Request<Body> {
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .body(Body::from(b"{}".as_ref()))
                .unwrap()
        }

        #[tokio::test]
        async fn ack_failure_returns_500() {
            let resp = ack_app(AckFailPublisher::failing())
                .oneshot(plain_request())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }

        #[tokio::test]
        async fn ack_timeout_returns_500() {
            let resp = ack_app(AckFailPublisher::hanging())
                .oneshot(plain_request())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    // ── serve_impl stream-creation failure ────────────────────────────────────

    #[derive(Clone)]
    struct TestJs {
        publisher: MockJetStreamPublisher,
        context: MockJetStreamContext,
    }

    impl JetStreamPublisher for TestJs {
        type PublishError = <MockJetStreamPublisher as JetStreamPublisher>::PublishError;
        type AckFuture = <MockJetStreamPublisher as JetStreamPublisher>::AckFuture;
        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: async_nats::HeaderMap,
            payload: bytes::Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            self.publisher
                .publish_with_headers(subject, headers, payload)
                .await
        }
    }

    impl JetStreamContext for TestJs {
        type Error = <MockJetStreamContext as JetStreamContext>::Error;
        type Stream = <MockJetStreamContext as JetStreamContext>::Stream;
        async fn get_or_create_stream<S: Into<async_nats::jetstream::stream::Config> + Send>(
            &self,
            config: S,
        ) -> Result<Self::Stream, Self::Error> {
            self.context.get_or_create_stream(config).await
        }
    }

    #[tokio::test]
    async fn serve_impl_returns_error_when_stream_creation_fails() {
        let js = TestJs {
            publisher: MockJetStreamPublisher::new(),
            context: MockJetStreamContext::new(),
        };
        js.context.fail_next();
        let result = serve_impl(test_config(), js).await;
        assert!(result.is_err());
    }
}
