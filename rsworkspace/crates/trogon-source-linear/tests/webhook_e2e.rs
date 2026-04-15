//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//! JetStream `publish_with_headers` also delivers to plain core-NATS subscribers,
//! so assertions use `nats.subscribe()` — no JetStream consumer setup needed.
//!
//! Run with:
//!   cargo test -p trogon-source-linear --test webhook_e2e

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
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
use trogon_source_linear::config::{LinearConfig, LinearWebhookSecret};
use trogon_source_linear::{provision, router};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

// ── Helpers ────────────────────────────────────────────────────────────────────

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

fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

const TEST_SECRET: &str = "int-test-secret";

fn test_config() -> LinearConfig {
    LinearConfig {
        webhook_secret: LinearWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("linear").expect("valid token"),
        stream_name: NatsToken::new("LINEAR").expect("valid token"),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).ok(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
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

    TestFixture {
        nats,
        app,
        _container: container,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Happy path: valid signature + valid payload → published to `linear.Issue.create`.
#[tokio::test]
async fn valid_webhook_publishes_to_jetstream_and_returns_200() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "Issue",
        "action": "create",
        "webhookId": "wh-e2e-001",
        "webhookTimestamp": now_ms(),
        "data": {}
    }))
    .unwrap();
    let sig = compute_sig(TEST_SECRET, &body);

    let mut sub = fixture.nats.subscribe("linear.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("linear-signature", &sig)
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

    assert_eq!(msg.subject.as_str(), "linear.Issue.create");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// Missing `linear-signature` header → 401, no NATS publish.
#[tokio::test]
async fn missing_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "Issue",
        "action": "create",
        "webhookTimestamp": now_ms()
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("linear.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
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

/// Wrong signature → 401, no NATS publish.
#[tokio::test]
async fn invalid_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "Issue",
        "action": "create",
        "webhookTimestamp": now_ms()
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("linear.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("linear-signature", "deadbeef")
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

/// Non-JSON body → `linear.unroutable` with reject reason `invalid_json`.
#[tokio::test]
async fn invalid_json_routes_to_unroutable_on_nats() {
    let fixture = setup().await;

    let body = b"not valid json";
    let sig = compute_sig(TEST_SECRET, body);

    let mut sub = fixture
        .nats
        .subscribe("linear.unroutable")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("linear-signature", &sig)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for unroutable NATS message")
        .expect("subscription closed");

    assert_eq!(msg.subject.as_str(), "linear.unroutable");
    let reason = msg
        .headers
        .as_ref()
        .and_then(|h| h.get("X-Linear-Reject-Reason"))
        .map(|v| v.as_str());
    assert_eq!(reason, Some("invalid_json"));
}

/// Very old `webhookTimestamp` → `linear.unroutable` with reason `stale_webhook_timestamp`.
#[tokio::test]
async fn stale_timestamp_routes_to_unroutable_on_nats() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "Issue",
        "action": "create",
        "webhookTimestamp": 1_000_000_000_000_u64
    }))
    .unwrap();
    let sig = compute_sig(TEST_SECRET, &body);

    let mut sub = fixture
        .nats
        .subscribe("linear.unroutable")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("linear-signature", &sig)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for unroutable NATS message")
        .expect("subscription closed");

    assert_eq!(msg.subject.as_str(), "linear.unroutable");
    let reason = msg
        .headers
        .as_ref()
        .and_then(|h| h.get("X-Linear-Reject-Reason"))
        .map(|v| v.as_str());
    assert_eq!(reason, Some("stale_webhook_timestamp"));
}

/// The `webhookId` is forwarded as `Nats-Msg-Id` for deduplication.
#[tokio::test]
async fn webhook_id_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "Comment",
        "action": "update",
        "webhookId": "dedup-key-xyz",
        "webhookTimestamp": now_ms(),
        "data": {}
    }))
    .unwrap();
    let sig = compute_sig(TEST_SECRET, &body);

    let mut sub = fixture.nats.subscribe("linear.Comment.update").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("linear-signature", &sig)
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
    assert_eq!(msg_id, Some("dedup-key-xyz"));
}
