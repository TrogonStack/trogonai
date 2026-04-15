//! Integration tests — real NATS JetStream + real HTTP router (webhook mode).
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//! JetStream `publish_with_headers` also delivers to plain core-NATS subscribers,
//! so assertions use `nats.subscribe()` — no JetStream consumer setup needed.
//!
//! Ed25519 key pairs are generated from fixed seeds for deterministic tests.
//!
//! Run with:
//!   cargo test -p trogon-source-discord --test webhook_e2e

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tower::ServiceExt as _;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, NatsJetStreamClient, NatsObjectStore, StreamMaxAge,
};
use trogon_source_discord::config::{DiscordConfig, SourceMode};
use trogon_source_discord::{provision, router};
use trogon_std::NonZeroDuration;

// ── Key pair helpers ───────────────────────────────────────────────────────────

fn test_keypair() -> (SigningKey, VerifyingKey) {
    let signing = SigningKey::from_bytes(&[2u8; 32]);
    let verifying = signing.verifying_key();
    (signing, verifying)
}

/// Sign `timestamp + body` with the Ed25519 signing key, return hex-encoded signature.
fn sign(signing_key: &SigningKey, timestamp: &str, body: &[u8]) -> String {
    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);
    hex::encode(signing_key.sign(&message).to_bytes())
}

const TIMESTAMP: &str = "1700000000";

// ── NATS helpers ───────────────────────────────────────────────────────────────

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

fn test_config(verifying_key: VerifyingKey) -> DiscordConfig {
    DiscordConfig {
        mode: SourceMode::Webhook {
            public_key: verifying_key,
        },
        subject_prefix: NatsToken::new("discord").unwrap(),
        stream_name: NatsToken::new("DISCORD").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
        nats_request_timeout: NonZeroDuration::from_secs(5).unwrap(),
    }
}

struct TestFixture {
    signing_key: SigningKey,
    nats: async_nats::Client,
    app: axum::Router,
    _container: ContainerAsync<Nats>,
}

async fn setup() -> TestFixture {
    let (signing_key, verifying_key) = test_keypair();

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
    let config = test_config(verifying_key);
    provision(&js_client, &config)
        .await
        .expect("stream provision failed");

    let publisher = ClaimCheckPublisher::new(
        js_client,
        object_store,
        "test-claims".to_string(),
        MaxPayload::from_server_limit(1024 * 1024),
    );

    // Discord router also needs a NATS client (for autocomplete request/reply)
    let app = router(publisher, nats.clone(), verifying_key, &config);

    TestFixture {
        signing_key,
        nats,
        app,
        _container: container,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// PING interaction (type=1) is answered inline with `{"type":1}` — no NATS publish.
#[tokio::test]
async fn ping_interaction_returns_pong_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({"type": 1})).unwrap();
    let sig = sign(&fixture.signing_key, TIMESTAMP, &body);

    let mut sub = fixture.nats.subscribe("discord.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Signature-Ed25519", &sig)
                .header("X-Signature-Timestamp", TIMESTAMP)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // Response body must be `{"type":1}`
    let resp_bytes = axum::body::to_bytes(resp.into_body(), 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["type"], 1);

    // No NATS message
    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "PING must not publish to NATS");
}

/// `application_command` (type=2) → published to `discord.application_command`, response `{"type":5}`.
#[tokio::test]
async fn application_command_publishes_to_jetstream_and_returns_deferred() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": 2,
        "id": "cmd-interaction-001",
        "data": { "name": "ping" }
    }))
    .unwrap();
    let sig = sign(&fixture.signing_key, TIMESTAMP, &body);

    let mut sub = fixture
        .nats
        .subscribe("discord.application_command")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Signature-Ed25519", &sig)
                .header("X-Signature-Timestamp", TIMESTAMP)
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // Response should be deferred channel message `{"type":5}`
    let resp_bytes = axum::body::to_bytes(resp.into_body(), 64).await.unwrap();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_json["type"], 5);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscription closed");

    assert_eq!(msg.subject.as_str(), "discord.application_command");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// `message_component` (type=3) → published to `discord.message_component`.
#[tokio::test]
async fn message_component_publishes_to_jetstream() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": 3,
        "id": "mc-interaction-002",
        "data": { "custom_id": "button_ok", "component_type": 2 }
    }))
    .unwrap();
    let sig = sign(&fixture.signing_key, TIMESTAMP, &body);

    let mut sub = fixture
        .nats
        .subscribe("discord.message_component")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Signature-Ed25519", &sig)
                .header("X-Signature-Timestamp", TIMESTAMP)
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

    assert_eq!(msg.subject.as_str(), "discord.message_component");
}

/// `modal_submit` (type=5) → published to `discord.modal_submit`.
#[tokio::test]
async fn modal_submit_publishes_to_jetstream() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": 5,
        "id": "modal-003",
        "data": { "custom_id": "feedback_modal", "components": [] }
    }))
    .unwrap();
    let sig = sign(&fixture.signing_key, TIMESTAMP, &body);

    let mut sub = fixture
        .nats
        .subscribe("discord.modal_submit")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Signature-Ed25519", &sig)
                .header("X-Signature-Timestamp", TIMESTAMP)
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

    assert_eq!(msg.subject.as_str(), "discord.modal_submit");
}

/// Missing `X-Signature-Ed25519` → 401, no NATS publish.
#[tokio::test]
async fn missing_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({"type": 2, "id": "x"})).unwrap();

    let mut sub = fixture.nats.subscribe("discord.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Signature-Timestamp", TIMESTAMP)
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

/// Wrong Ed25519 signature → 401, no NATS publish.
#[tokio::test]
async fn wrong_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({"type": 2, "id": "x"})).unwrap();

    // Sign with a different key
    let (other_key, _) = {
        let sk = SigningKey::from_bytes(&[99u8; 32]);
        let vk = sk.verifying_key();
        (sk, vk)
    };
    let bad_sig = sign(&other_key, TIMESTAMP, &body);

    let mut sub = fixture.nats.subscribe("discord.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Signature-Ed25519", &bad_sig)
                .header("X-Signature-Timestamp", TIMESTAMP)
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

/// Unknown interaction type → `discord.unroutable` with reject reason.
#[tokio::test]
async fn unknown_interaction_type_routes_to_unroutable() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": 99,
        "id": "unknown-001"
    }))
    .unwrap();
    let sig = sign(&fixture.signing_key, TIMESTAMP, &body);

    let mut sub = fixture
        .nats
        .subscribe("discord.unroutable")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Signature-Ed25519", &sig)
                .header("X-Signature-Timestamp", TIMESTAMP)
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

    assert_eq!(msg.subject.as_str(), "discord.unroutable");
    let reason = msg
        .headers
        .as_ref()
        .and_then(|h| h.get("X-Discord-Reject-Reason"))
        .map(|v| v.as_str());
    assert_eq!(reason, Some("unknown_interaction_type"));
}

/// `interaction_id` is forwarded as `Nats-Msg-Id` for deduplication.
#[tokio::test]
async fn interaction_id_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": 2,
        "id": "dedup-cmd-999",
        "data": { "name": "help" }
    }))
    .unwrap();
    let sig = sign(&fixture.signing_key, TIMESTAMP, &body);

    let mut sub = fixture
        .nats
        .subscribe("discord.application_command")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Signature-Ed25519", &sig)
                .header("X-Signature-Timestamp", TIMESTAMP)
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
    assert_eq!(msg_id, Some("dedup-cmd-999"));
}
