//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//! JetStream `publish_with_headers` also delivers to plain core-NATS subscribers,
//! so assertions use `nats.subscribe()` — no JetStream consumer setup needed.
//!
//! Run with:
//!   cargo test -p trogon-source-github --test webhook_e2e

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
use trogon_source_github::config::{GitHubWebhookSecret, GithubConfig};
use trogon_source_github::{provision, router};
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

/// Compute `sha256=<hex-hmac-sha256>` as GitHub does.
fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

const TEST_SECRET: &str = "gh-int-test-secret";

fn test_config() -> GithubConfig {
    GithubConfig {
        webhook_secret: GitHubWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("github").unwrap(),
        stream_name: NatsToken::new("GITHUB").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
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

/// Happy path: valid HMAC signature + push event → published to `github.push`.
#[tokio::test]
async fn valid_push_event_publishes_to_jetstream_and_returns_200() {
    let fixture = setup().await;

    let body = br#"{"ref":"refs/heads/main","repository":{"full_name":"org/repo"}}"#;
    let sig = compute_sig(TEST_SECRET, body);

    let mut sub = fixture.nats.subscribe("github.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Hub-Signature-256", &sig)
                .header("X-GitHub-Event", "push")
                .header("X-GitHub-Delivery", "delivery-abc-123")
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

    assert_eq!(msg.subject.as_str(), "github.push");
    assert_eq!(msg.payload.as_ref(), &body[..]);
}

/// Missing `X-Hub-Signature-256` → 401, no NATS publish.
#[tokio::test]
async fn missing_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let mut sub = fixture.nats.subscribe("github.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-GitHub-Event", "push")
                .body(Body::from(b"{}".to_vec()))
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

/// Wrong HMAC signature → 401, no NATS publish.
#[tokio::test]
async fn wrong_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let mut sub = fixture.nats.subscribe("github.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Hub-Signature-256", "sha256=deadbeef")
                .header("X-GitHub-Event", "push")
                .body(Body::from(b"{}".to_vec()))
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

/// Missing `X-GitHub-Event` header → 400, no NATS publish (GitHub doesn't route to unroutable).
#[tokio::test]
async fn missing_event_header_returns_400_no_nats_message() {
    let fixture = setup().await;

    let body = b"{}";
    let sig = compute_sig(TEST_SECRET, body);

    let mut sub = fixture.nats.subscribe("github.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Hub-Signature-256", &sig)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "expected no NATS message on missing event header");
}

/// `X-GitHub-Delivery` is forwarded as `Nats-Msg-Id` for deduplication.
#[tokio::test]
async fn delivery_id_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let body = br#"{"action":"opened"}"#;
    let sig = compute_sig(TEST_SECRET, body);

    let mut sub = fixture
        .nats
        .subscribe("github.pull_request")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Hub-Signature-256", &sig)
                .header("X-GitHub-Event", "pull_request")
                .header("X-GitHub-Delivery", "dedup-delivery-xyz")
                .body(Body::from(body.to_vec()))
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
    assert_eq!(msg_id, Some("dedup-delivery-xyz"));

    let event_header = msg
        .headers
        .as_ref()
        .and_then(|h| h.get("X-GitHub-Event"))
        .map(|v| v.as_str());
    assert_eq!(event_header, Some("pull_request"));
}
