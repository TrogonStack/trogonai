//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-source-incidentio --test webhook_e2e

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::StreamExt as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tower::ServiceExt as _;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, NatsJetStreamClient, NatsObjectStore, StreamMaxAge,
};
use trogon_source_incidentio::{IncidentioConfig, IncidentioSigningSecret, provision, router};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

// "whsec_" + base64("test-secret") — secret.as_bytes() returns b"test-secret"
const TEST_SECRET: &str = "whsec_dGVzdC1zZWNyZXQ=";

// ── Helpers ────────────────────────────────────────────────────────────────────

fn compute_sig(body: &[u8], webhook_id: &str, timestamp: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(b"test-secret").unwrap();
    let mut content = Vec::new();
    content.extend_from_slice(webhook_id.as_bytes());
    content.push(b'.');
    content.extend_from_slice(timestamp.as_bytes());
    content.push(b'.');
    content.extend_from_slice(body);
    mac.update(&content);
    format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
}

fn current_ts() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}

fn test_config() -> IncidentioConfig {
    IncidentioConfig {
        signing_secret: IncidentioSigningSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("incidentio").unwrap(),
        stream_name: NatsToken::new("INCIDENTIO").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    }
}

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect failed")
}

struct TestFixture {
    nats: async_nats::Client,
    app: axum::Router,
    _container: ContainerAsync<Nats>,
}

async fn setup() -> TestFixture {
    let (container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(nats.clone());

    let object_store = NatsObjectStore::provision(
        &js,
        async_nats::jetstream::object_store::Config {
            bucket: "test-claims".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("object store provision failed");

    let js_client = NatsJetStreamClient::new(js);
    let config = test_config();
    provision(&js_client, &config)
        .await
        .expect("stream provision failed");

    let publisher = ClaimCheckPublisher::new(
        js_client,
        object_store,
        "test-claims".to_string(),
        MaxPayload::from_server_limit(1024 * 1024),
    );

    let app = router(publisher, &config);
    TestFixture { nats, app, _container: container }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Happy path: valid event publishes to the correct NATS subject.
#[tokio::test]
async fn valid_incident_event_publishes_to_nats_and_returns_200() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = serde_json::to_vec(&serde_json::json!({
        "event_type": "public_incident.incident_created_v2",
        "data": { "id": "01ABC" }
    }))
    .unwrap();
    let sig = compute_sig(&body, "msg-int-1", &ts);

    let mut sub = fixture.nats.subscribe("incidentio.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("webhook-id", "msg-int-1")
                .header("webhook-timestamp", &ts)
                .header("webhook-signature", &sig)
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscription closed");

    assert_eq!(
        msg.subject.as_str(),
        "incidentio.public_incident.incident_created_v2"
    );
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// Missing all three signature headers → 401.
#[tokio::test]
async fn missing_signature_returns_401() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = b"{}";

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("webhook-id", "msg-int-2")
                .header("webhook-timestamp", &ts)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Wrong signature → 401, no NATS publish.
#[tokio::test]
async fn wrong_signature_returns_401_and_no_nats_message() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = serde_json::to_vec(&serde_json::json!({
        "event_type": "public_incident.incident_created_v2"
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("incidentio.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("webhook-id", "msg-int-3")
                .header("webhook-timestamp", &ts)
                .header("webhook-signature", "v1,d3JvbmdzaWduYXR1cmU=")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "expected no NATS message on auth failure");
}

/// Stale timestamp → 401, no NATS publish.
#[tokio::test]
async fn stale_timestamp_returns_401_and_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "event_type": "public_incident.incident_created_v2"
    }))
    .unwrap();

    let old_ts = "1000000000";
    let sig = compute_sig(&body, "msg-int-4", old_ts);

    let mut sub = fixture.nats.subscribe("incidentio.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("webhook-id", "msg-int-4")
                .header("webhook-timestamp", old_ts)
                .header("webhook-signature", &sig)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "expected no NATS message on stale timestamp");
}

/// Invalid JSON → published to `incidentio.unroutable`, HTTP 200.
#[tokio::test]
async fn invalid_json_publishes_to_unroutable_and_returns_200() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = b"{not-json";
    let sig = compute_sig(body, "msg-int-5", &ts);

    let mut sub = fixture.nats.subscribe("incidentio.unroutable").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("webhook-id", "msg-int-5")
                .header("webhook-timestamp", &ts)
                .header("webhook-signature", &sig)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscription closed");

    assert_eq!(msg.subject.as_str(), "incidentio.unroutable");
}

/// `webhook-id` is forwarded as `Nats-Msg-Id` for deduplication.
#[tokio::test]
async fn webhook_id_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = serde_json::to_vec(&serde_json::json!({
        "event_type": "public_incident.incident_created_v2",
        "data": { "id": "01DEF" }
    }))
    .unwrap();
    let webhook_id = "msg-dedup-999";
    let sig = compute_sig(&body, webhook_id, &ts);

    let mut sub = fixture.nats.subscribe("incidentio.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("webhook-id", webhook_id)
                .header("webhook-timestamp", &ts)
                .header("webhook-signature", &sig)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    let msg_id = msg
        .headers
        .as_ref()
        .and_then(|h| h.get("Nats-Msg-Id"))
        .map(|v| v.as_str());
    assert_eq!(msg_id, Some(webhook_id));
}
