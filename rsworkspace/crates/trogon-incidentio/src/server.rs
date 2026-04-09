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

use std::future::IntoFuture as _;
use std::net::SocketAddr;
use std::time::Duration;

use async_nats::jetstream::stream;
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
use crate::store::IncidentRepository;
use trogon_nats::jetstream::{JetStreamContext, JetStreamPublisher};

// ── Application state ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState<J, R> {
    js: J,
    incident_store: R,
    webhook_secret: Option<String>,
    subject_prefix: String,
    ack_timeout: Duration,
}

// ── Server entry-point ────────────────────────────────────────────────────────

/// Start the incident.io webhook HTTP server.
///
/// Ensures JetStream stream and KV bucket exist, then listens for:
/// - `POST /webhook` — incident.io events → NATS JetStream + KV update
/// - `GET  /health`  — liveness probe
/// - `GET  /incidents` — list all stored incidents
/// - `GET  /incidents/:id` — get a single stored incident
#[cfg(not(coverage))]
pub async fn serve(
    config: IncidentioConfig,
    nats: async_nats::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::store::IncidentStore;
    use async_nats::jetstream;
    use trogon_nats::jetstream::NatsJetStreamClient;

    let js = NatsJetStreamClient::new(jetstream::new(nats));
    let incident_store = IncidentStore::open(js.context())
        .await
        .map_err(|e| format!("Failed to open IncidentStore: {e}"))?;
    serve_impl(config, js, incident_store).await
}

async fn serve_impl<J, R>(
    config: IncidentioConfig,
    js: J,
    incident_store: R,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    J: JetStreamPublisher + JetStreamContext + Clone + Send + Sync + 'static,
    <J as JetStreamContext>::Error: 'static,
    R: IncidentRepository,
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
        incident_store,
        webhook_secret: config.webhook_secret,
        subject_prefix: config.subject_prefix,
        ack_timeout: Duration::from_secs(10),
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook::<J, R>))
        .route("/health", get(handle_health))
        .route("/incidents", get(list_incidents::<J, R>))
        .route("/incidents/:id", get(get_incident_by_id::<J, R>))
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
async fn handle_webhook<J, R>(
    State(state): State<AppState<J, R>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode
where
    J: JetStreamPublisher + Clone + Send + Sync + 'static,
    R: IncidentRepository,
{
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

    match state
        .js
        .publish_with_headers(subject.clone(), nats_headers, body)
        .await
    {
        Ok(ack_future) => {
            match tokio::time::timeout(state.ack_timeout, ack_future.into_future()).await {
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
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to publish incident.io event to NATS");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// `GET /incidents` — returns all stored incidents as a JSON array.
async fn list_incidents<J, R>(
    State(state): State<AppState<J, R>>,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode>
where
    J: Clone + Send + Sync + 'static,
    R: IncidentRepository,
{
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
async fn get_incident_by_id<J, R>(
    State(state): State<AppState<J, R>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode>
where
    J: Clone + Send + Sync + 'static,
    R: IncidentRepository,
{
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
    use super::*;
    use crate::events::nats_subject_suffix;
    use crate::store::mock::MockIncidentStore;
    use axum::body::Body;
    use axum::http::Request;
    use bytes::Bytes;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use tower::ServiceExt as _;
    use trogon_nats::jetstream::{MockJetStreamContext, MockJetStreamPublisher};

    type HmacSha256 = Hmac<Sha256>;

    const TEST_SECRET: &str = "inc-test-secret";

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn test_config() -> IncidentioConfig {
        IncidentioConfig {
            webhook_secret: Some(TEST_SECRET.to_string()),
            api_token: None,
            port: 0,
            subject_prefix: "incidentio".to_string(),
            stream_name: "INCIDENTIO".to_string(),
            stream_max_age: Duration::from_secs(3600),
            nats: trogon_nats::NatsConfig::from_env(&trogon_std::env::InMemoryEnv::new()),
        }
    }

    fn mock_app(publisher: MockJetStreamPublisher, store: MockIncidentStore) -> Router {
        let config = test_config();
        let state = AppState {
            js: publisher,
            incident_store: store,
            webhook_secret: config.webhook_secret,
            subject_prefix: config.subject_prefix,
            ack_timeout: Duration::from_secs(10),
        };
        Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<MockJetStreamPublisher, MockIncidentStore>),
            )
            .route("/health", get(handle_health))
            .route(
                "/incidents",
                get(list_incidents::<MockJetStreamPublisher, MockIncidentStore>),
            )
            .route(
                "/incidents/:id",
                get(get_incident_by_id::<MockJetStreamPublisher, MockIncidentStore>),
            )
            .with_state(state)
    }

    fn webhook_request(body: &[u8], sig: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("x-incident-delivery", "del-1");
        if let Some(s) = sig {
            builder = builder.header("x-incident-signature", s);
        }
        builder.body(Body::from(body.to_vec())).unwrap()
    }

    // ── event_type helpers ────────────────────────────────────────────────────

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

    // ── handler tests ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn health_returns_200() {
        let app = mock_app(MockJetStreamPublisher::new(), MockIncidentStore::new());
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
        let store = MockIncidentStore::new();
        let app = mock_app(publisher.clone(), store.clone());
        let body = br#"{"event_type":"incident.created","incident":{"id":"inc-42"}}"#;
        let sig = compute_sig(TEST_SECRET, body);

        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let msgs = publisher.published_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].subject, "incidentio.incident.created");
        assert_eq!(msgs[0].payload, Bytes::from(body.as_ref()));
        // KV store also updated
        assert!(store.snapshot().contains_key("inc-42"));
    }

    #[tokio::test]
    async fn invalid_signature_returns_401_and_does_not_publish() {
        let publisher = MockJetStreamPublisher::new();
        let app = mock_app(publisher.clone(), MockIncidentStore::new());
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
        let app = mock_app(publisher.clone(), MockIncidentStore::new());
        let resp = app.oneshot(webhook_request(b"{}", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn no_secret_accepts_without_signature() {
        let publisher = MockJetStreamPublisher::new();
        let state = AppState {
            js: publisher.clone(),
            incident_store: MockIncidentStore::new(),
            webhook_secret: None,
            subject_prefix: "incidentio".to_string(),
            ack_timeout: Duration::from_secs(10),
        };
        let app = Router::new()
            .route(
                "/webhook",
                post(handle_webhook::<MockJetStreamPublisher, MockIncidentStore>),
            )
            .with_state(state);
        let body = br#"{"event_type":"incident.updated","incident":{"id":"inc-1"}}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(Body::from(body.as_ref()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            publisher.published_subjects(),
            vec!["incidentio.incident.updated"]
        );
    }

    #[tokio::test]
    async fn publish_failure_returns_500() {
        let publisher = MockJetStreamPublisher::new();
        publisher.fail_next_js_publish();
        let app = mock_app(publisher.clone(), MockIncidentStore::new());
        let body = b"{}";
        let sig = compute_sig(TEST_SECRET, body);
        let resp = app
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn list_incidents_returns_stored_incidents() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockIncidentStore::new();
        let body =
            br#"{"event_type":"incident.created","incident":{"id":"inc-1","name":"db down"}}"#;
        let sig = compute_sig(TEST_SECRET, body);
        let app = mock_app(publisher, store);

        // POST — stores the incident
        app.clone()
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .unwrap();

        // GET /incidents — same shared store via Arc
        let req = Request::builder()
            .method("GET")
            .uri("/incidents")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["incident"]["id"], "inc-1");
    }

    #[tokio::test]
    async fn get_incident_by_id_returns_404_when_not_found() {
        let app = mock_app(MockJetStreamPublisher::new(), MockIncidentStore::new());
        let req = Request::builder()
            .method("GET")
            .uri("/incidents/missing-id")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_incident_by_id_returns_stored_incident() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockIncidentStore::new();
        let body =
            br#"{"event_type":"incident.created","incident":{"id":"inc-7","name":"svc down"}}"#;
        let sig = compute_sig(TEST_SECRET, body);
        let app = mock_app(publisher, store);

        // POST — stores the incident
        app.clone()
            .oneshot(webhook_request(body, Some(&sig)))
            .await
            .unwrap();

        // GET /incidents/inc-7
        let req = Request::builder()
            .method("GET")
            .uri("/incidents/inc-7")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["incident"]["id"], "inc-7");
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
                incident_store: MockIncidentStore::new(),
                webhook_secret: None,
                subject_prefix: "incidentio".to_string(),
                ack_timeout: Duration::from_millis(50),
            };
            Router::new()
                .route(
                    "/webhook",
                    post(handle_webhook::<AckFailPublisher, MockIncidentStore>),
                )
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
        let result = serve_impl(test_config(), js, MockIncidentStore::new()).await;
        assert!(result.is_err());
    }
}
