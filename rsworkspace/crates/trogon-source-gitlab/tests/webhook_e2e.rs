//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//! JetStream `publish_with_headers` also delivers to plain core-NATS subscribers,
//! so assertions use `nats.subscribe()` — no JetStream consumer setup needed.
//!
//! Run with:
//!   cargo test -p trogon-source-gitlab --test webhook_e2e

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tower::ServiceExt as _;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, NatsJetStreamClient, NatsObjectStore, StreamMaxAge,
};
use trogon_source_gitlab::config::{GitLabWebhookSecret, GitlabConfig};
use trogon_source_gitlab::{provision, router};
use trogon_std::NonZeroDuration;

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

const TEST_SECRET: &str = "gl-int-test-secret";

fn test_config() -> GitlabConfig {
    GitlabConfig {
        webhook_secret: GitLabWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("gitlab").unwrap(),
        stream_name: NatsToken::new("GITLAB").unwrap(),
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

/// Happy path: valid token + push event → published to `gitlab.push`.
#[tokio::test]
async fn valid_push_event_publishes_to_jetstream_and_returns_200() {
    let fixture = setup().await;

    let body = br#"{"ref":"refs/heads/main","repository":{"full_name":"org/repo"}}"#;

    let mut sub = fixture.nats.subscribe("gitlab.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-gitlab-token", TEST_SECRET)
                .header("x-gitlab-event", "push")
                .header("x-gitlab-webhook-uuid", "uuid-test")
                .header("idempotency-key", "idem-key-test")
                .header("x-gitlab-instance", "https://gitlab.example.com")
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

    assert_eq!(msg.subject.as_str(), "gitlab.push");
    assert_eq!(msg.payload.as_ref(), &body[..]);
}

/// Missing `X-Gitlab-Token` → 401, no NATS publish.
#[tokio::test]
async fn missing_token_returns_401_no_nats_message() {
    let fixture = setup().await;

    let mut sub = fixture.nats.subscribe("gitlab.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-gitlab-event", "push")
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

/// Wrong token → 401, no NATS publish.
#[tokio::test]
async fn wrong_token_returns_401_no_nats_message() {
    let fixture = setup().await;

    let mut sub = fixture.nats.subscribe("gitlab.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-gitlab-token", "wrong-token")
                .header("x-gitlab-event", "push")
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

/// Missing `X-GitLab-Event` header → `gitlab.unroutable` with reason `missing_event_header`.
/// GitLab unroutable returns 200 (to prevent GitLab from auto-disabling the webhook).
#[tokio::test]
async fn missing_event_header_routes_to_unroutable_returns_200() {
    let fixture = setup().await;

    let mut sub = fixture
        .nats
        .subscribe("gitlab.unroutable")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-gitlab-token", TEST_SECRET)
                .body(Body::from(b"{}".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    // GitLab unroutable returns 200 to avoid webhook auto-disable
    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for unroutable NATS message")
        .expect("subscription closed");

    assert_eq!(msg.subject.as_str(), "gitlab.unroutable");
    let reason = msg
        .headers
        .as_ref()
        .and_then(|h| h.get("X-GitLab-Reject-Reason"))
        .map(|v| v.as_str());
    assert_eq!(reason, Some("missing_event_header"));
}

/// Event with spaces in header is normalized to underscores in subject.
#[tokio::test]
async fn event_header_with_spaces_normalizes_to_underscores() {
    let fixture = setup().await;

    let mut sub = fixture
        .nats
        .subscribe("gitlab.merge_request_hook")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-gitlab-token", TEST_SECRET)
                .header("x-gitlab-event", "Merge Request Hook")
                .body(Body::from(b"{}".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    assert_eq!(msg.subject.as_str(), "gitlab.merge_request_hook");
}

/// `Idempotency-Key` is forwarded as `Nats-Msg-Id` for deduplication.
#[tokio::test]
async fn idempotency_key_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let mut sub = fixture.nats.subscribe("gitlab.issues").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-gitlab-token", TEST_SECRET)
                .header("x-gitlab-event", "issues")
                .header("idempotency-key", "my-dedup-key-123")
                .body(Body::from(b"{}".to_vec()))
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
    assert_eq!(msg_id, Some("my-dedup-key-123"));
}
