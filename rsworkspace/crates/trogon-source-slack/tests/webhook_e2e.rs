//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//! JetStream `publish_with_headers` also delivers to plain core-NATS subscribers,
//! so assertions use `nats.subscribe()` — no JetStream consumer setup needed.
//!
//! Run with:
//!   cargo test -p trogon-source-slack --test webhook_e2e

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
use trogon_source_slack::config::{SlackConfig, SlackSigningSecret};
use trogon_source_slack::{provision, router};
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

/// Computes `v0=<hex-hmac-sha256("v0:{ts}:{body}")>` as Slack does.
fn compute_slack_sig(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);
    format!("v0={}", hex::encode(mac.finalize().into_bytes()))
}

fn current_ts() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}

const TEST_SECRET: &str = "slack-int-test-secret";

fn test_config() -> SlackConfig {
    SlackConfig {
        signing_secret: SlackSigningSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("slack").unwrap(),
        stream_name: NatsToken::new("SLACK").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
        timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
    }
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

/// Happy path: valid signature + event_callback → published to `slack.message`.
#[tokio::test]
async fn valid_event_callback_publishes_to_jetstream_and_returns_200() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev0123456789",
        "team_id": "T01234567",
        "event": {
            "type": "message",
            "text": "hello world"
        }
    }))
    .unwrap();
    let sig = compute_slack_sig(TEST_SECRET, &ts, &body);

    let mut sub = fixture.nats.subscribe("slack.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-slack-signature", &sig)
                .header("x-slack-request-timestamp", &ts)
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

    assert_eq!(msg.subject.as_str(), "slack.event.message");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// Missing `X-Slack-Signature` header → 401.
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
                .header("x-slack-request-timestamp", &ts)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Missing `X-Slack-Request-Timestamp` header → 401.
#[tokio::test]
async fn missing_timestamp_returns_401() {
    let fixture = setup().await;

    let body = b"{}";
    let sig = compute_slack_sig(TEST_SECRET, "0", body);

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-slack-signature", &sig)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Wrong signature → 401, no NATS publish.
#[tokio::test]
async fn wrong_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev001",
        "team_id": "T001",
        "event": { "type": "message" }
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("slack.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-slack-signature", "v0=deadbeef")
                .header("x-slack-request-timestamp", &ts)
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

/// `url_verification` challenge is answered inline without publishing to NATS.
#[tokio::test]
async fn url_verification_returns_challenge_no_nats_message() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "url_verification",
        "challenge": "my-challenge-token",
        "token": "deprecated"
    }))
    .unwrap();
    let sig = compute_slack_sig(TEST_SECRET, &ts, &body);

    let mut sub = fixture.nats.subscribe("slack.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-slack-signature", &sig)
                .header("x-slack-request-timestamp", &ts)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // Challenge text is returned in the response body
    let resp_body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(resp_body.as_ref(), b"my-challenge-token");

    // No NATS message
    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "url_verification must not publish to NATS");
}

/// Stale timestamp (very old) → 401 before any NATS publish.
#[tokio::test]
async fn stale_timestamp_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev001",
        "team_id": "T001",
        "event": { "type": "message" }
    }))
    .unwrap();

    // Timestamp from the distant past
    let old_ts = "1000000000";
    let sig = compute_slack_sig(TEST_SECRET, old_ts, &body);

    let mut sub = fixture.nats.subscribe("slack.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-slack-signature", &sig)
                .header("x-slack-request-timestamp", old_ts)
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

/// `event_id` is forwarded as `Nats-Msg-Id` for deduplication.
#[tokio::test]
async fn event_id_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let ts = current_ts();
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "event_callback",
        "event_id": "Ev-dedup-999",
        "team_id": "T001",
        "event": {
            "type": "app_mention",
            "text": "hello"
        }
    }))
    .unwrap();
    let sig = compute_slack_sig(TEST_SECRET, &ts, &body);

    let mut sub = fixture
        .nats
        .subscribe("slack.event.app_mention")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-slack-signature", &sig)
                .header("x-slack-request-timestamp", &ts)
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
    assert_eq!(msg_id, Some("Ev-dedup-999"));
}
