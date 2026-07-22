//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-source-notion --test webhook_e2e

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
use trogon_source_notion::{NotionConfig, NotionVerificationToken, provision, router};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

const TEST_VERIFICATION_TOKEN: &str = "notion-int-test-token";

// ── Helpers ────────────────────────────────────────────────────────────────────

fn compute_sig(token: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(token.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn test_config() -> NotionConfig {
    NotionConfig {
        verification_token: NotionVerificationToken::new(TEST_VERIFICATION_TOKEN).unwrap(),
        subject_prefix: NatsToken::new("notion").unwrap(),
        stream_name: NatsToken::new("NOTION").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
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

/// Happy path: valid signed event publishes to the correct NATS subject.
#[tokio::test]
async fn valid_event_publishes_to_nats_and_returns_200() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "id": "367cba44-b6f3-4c92-81e7-6a2e9659efd4",
        "subscription_id": "29d75c0d-5546-4414-8459-7b7a92f1fc4b",
        "attempt_number": 1,
        "type": "page.created",
        "entity": {
            "id": "153104cd-477e-809d-8dc4-ff2d96ae3090",
            "type": "page"
        }
    }))
    .unwrap();
    let sig = compute_sig(TEST_VERIFICATION_TOKEN, &body);

    let mut sub = fixture.nats.subscribe("notion.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Notion-Signature", &sig)
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

    assert_eq!(msg.subject.as_str(), "notion.page.created");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// Bootstrap verification request (no signature, no `type`) publishes to subscription.verification.
#[tokio::test]
async fn bootstrap_verification_request_publishes_to_nats_and_returns_200() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "verification_token": TEST_VERIFICATION_TOKEN
    }))
    .unwrap();

    let mut sub = fixture
        .nats
        .subscribe("notion.subscription.verification")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
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

    assert_eq!(msg.subject.as_str(), "notion.subscription.verification");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// Missing `X-Notion-Signature` on a regular event → 401, no NATS publish.
#[tokio::test]
async fn missing_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "page.created",
        "id": "evt-1"
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("notion.>").await.unwrap();

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
async fn wrong_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "page.created",
        "id": "evt-2"
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("notion.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Notion-Signature", "sha256=deadbeef")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "expected no NATS message on wrong signature");
}

/// Unroutable event (invalid event type) publishes to `notion.unroutable`.
#[tokio::test]
async fn invalid_event_type_publishes_to_unroutable_and_returns_200() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "id": "evt-3",
        "type": "invalid event type with spaces"
    }))
    .unwrap();
    let sig = compute_sig(TEST_VERIFICATION_TOKEN, &body);

    let mut sub = fixture.nats.subscribe("notion.unroutable").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Notion-Signature", &sig)
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

    assert_eq!(msg.subject.as_str(), "notion.unroutable");
}

/// Event `id` forwarded as `Nats-Msg-Id` for deduplication.
#[tokio::test]
async fn event_id_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "id": "dedup-evt-999",
        "subscription_id": "sub-1",
        "attempt_number": 1,
        "type": "comment.created",
        "entity": { "id": "block-1", "type": "block" }
    }))
    .unwrap();
    let sig = compute_sig(TEST_VERIFICATION_TOKEN, &body);

    let mut sub = fixture.nats.subscribe("notion.comment.created").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("X-Notion-Signature", &sig)
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
    assert_eq!(msg_id, Some("dedup-evt-999"));
}
