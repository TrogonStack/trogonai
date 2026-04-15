//! Integration tests — trogon-gateway nested routing.
//!
//! Verifies that the gateway correctly nests source routers at their respective
//! paths (/github/webhook, /linear/webhook) and that liveness/readiness probes work.
//! Since the gateway is a [[bin]] crate with no [lib], the combined router is built
//! directly from the source crates' public APIs — matching what mount_sources() does.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-gateway --test routes_e2e

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
use trogon_source_linear::config::{LinearConfig, LinearWebhookSecret};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

// ── Constants ──────────────────────────────────────────────────────────────────

const GH_SECRET: &str = "gw-gh-test-secret";
const LINEAR_SECRET: &str = "gw-linear-test-secret";

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

fn gh_sig(body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(GH_SECRET.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn linear_sig(body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(LINEAR_SECRET.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn github_config() -> GithubConfig {
    GithubConfig {
        webhook_secret: GitHubWebhookSecret::new(GH_SECRET).unwrap(),
        subject_prefix: NatsToken::new("github").unwrap(),
        stream_name: NatsToken::new("GITHUB").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
    }
}

fn linear_config() -> LinearConfig {
    LinearConfig {
        webhook_secret: LinearWebhookSecret::new(LINEAR_SECRET).unwrap(),
        subject_prefix: NatsToken::new("linear").unwrap(),
        stream_name: NatsToken::new("LINEAR").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).ok(),
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

    let gh_cfg = github_config();
    let lin_cfg = linear_config();

    trogon_source_github::provision(&js_client, &gh_cfg)
        .await
        .expect("github stream provision failed");
    trogon_source_linear::provision(&js_client, &lin_cfg)
        .await
        .expect("linear stream provision failed");

    let publisher = ClaimCheckPublisher::new(
        js_client,
        object_store,
        "test-claims".to_string(),
        MaxPayload::from_server_limit(1024 * 1024),
    );

    // Mirror mount_sources() from http.rs, but using only GitHub and Linear to keep
    // the test focused and avoid provisioning every source's stream.
    let app = axum::Router::new()
        .route(
            "/-/liveness",
            axum::routing::get(|| async { StatusCode::OK }),
        )
        .route(
            "/-/readiness",
            axum::routing::get(|| async { StatusCode::OK }),
        )
        .nest(
            "/github",
            trogon_source_github::router(publisher.clone(), &gh_cfg),
        )
        .nest(
            "/linear",
            trogon_source_linear::router(publisher.clone(), &lin_cfg),
        );

    TestFixture {
        nats,
        app,
        _container: container,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Liveness and readiness probes both return 200.
#[tokio::test]
async fn liveness_and_readiness_return_200() {
    let fixture = setup().await;

    let liveness = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/-/liveness")
                .body(Body::from(vec![]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(liveness.status(), StatusCode::OK);

    let readiness = fixture
        .app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/-/readiness")
                .body(Body::from(vec![]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(readiness.status(), StatusCode::OK);
}

/// GitHub webhook at /github/webhook publishes to the github.* NATS subject.
/// This verifies the nested routing: the source router receives the request at
/// /webhook (not /github/webhook) after the gateway strips the prefix.
#[tokio::test]
async fn github_webhook_at_nested_path_publishes_to_nats() {
    let fixture = setup().await;

    let body = br#"{"ref":"refs/heads/main","repository":{"full_name":"org/repo"}}"#;
    let sig = gh_sig(body);

    let mut sub = fixture.nats.subscribe("github.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/github/webhook")
                .header("X-Hub-Signature-256", &sig)
                .header("X-GitHub-Event", "push")
                .header("X-GitHub-Delivery", "gw-delivery-001")
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
}

/// Linear webhook at /linear/webhook publishes to the linear.* NATS subject.
#[tokio::test]
async fn linear_webhook_at_nested_path_publishes_to_nats() {
    let fixture = setup().await;

    let ts = now_ms();
    let body = format!(
        r#"{{"type":"Issue","action":"create","webhookId":"gw-lin-001","webhookTimestamp":{ts},"data":{{}}}}"#
    )
    .into_bytes();
    let sig = linear_sig(&body);

    let mut sub = fixture.nats.subscribe("linear.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/linear/webhook")
                .header("linear-signature", &sig)
                .body(Body::from(body))
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
}

/// A valid GitHub-signed request sent to /linear/webhook is rejected (401).
/// Confirms sources are isolated: GitHub HMAC is not accepted by the Linear handler.
#[tokio::test]
async fn github_auth_at_linear_path_is_rejected() {
    let fixture = setup().await;

    let body = br#"{"ref":"refs/heads/main"}"#;
    // Sign with the GitHub secret — this is wrong for the Linear handler.
    let gh_signature = gh_sig(body);

    let mut sub = fixture.nats.subscribe("linear.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/linear/webhook")
                .header("linear-signature", &gh_signature)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "rejected request must not publish to NATS");
}

/// Unknown top-level path returns 404.
#[tokio::test]
async fn unknown_path_returns_404() {
    let fixture = setup().await;

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/unknown/webhook")
                .body(Body::from(vec![]))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
