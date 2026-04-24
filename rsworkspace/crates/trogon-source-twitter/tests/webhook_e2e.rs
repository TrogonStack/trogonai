//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-source-twitter --test webhook_e2e

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
use trogon_source_twitter::{TwitterConfig, TwitterConsumerSecret, provision, router};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

const TEST_SECRET: &str = "twitter-int-test-secret";

// ── Helpers ────────────────────────────────────────────────────────────────────

fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", STANDARD.encode(mac.finalize().into_bytes()))
}

fn test_config() -> TwitterConfig {
    TwitterConfig {
        consumer_secret: TwitterConsumerSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("twitter").unwrap(),
        stream_name: NatsToken::new("TWITTER").unwrap(),
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

/// CRC challenge GET returns the correct `response_token`.
#[tokio::test]
async fn crc_challenge_returns_correct_response_token() {
    let fixture = setup().await;

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/webhook?crc_token=test-challenge")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let expected_sig = compute_sig(TEST_SECRET, b"test-challenge");
    assert_eq!(payload["response_token"], expected_sig);
}

/// Happy path: valid X Activity event publishes to the correct NATS subject.
#[tokio::test]
async fn valid_x_activity_event_publishes_to_nats_and_returns_200() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "data": {
            "event_type": "profile.update.bio",
            "payload": {
                "before": "before text",
                "after": "after text"
            }
        }
    }))
    .unwrap();
    let sig = compute_sig(TEST_SECRET, &body);

    let mut sub = fixture.nats.subscribe("twitter.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-twitter-webhooks-signature", &sig)
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

    assert_eq!(msg.subject.as_str(), "twitter.profile.update.bio");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// Account activity event (favorite) publishes to `twitter.favorite_events`.
#[tokio::test]
async fn account_activity_event_publishes_to_correct_subject() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "for_user_id": "2244994945",
        "favorite_events": [{
            "id": "fav-int-1",
            "created_at": "Mon Mar 26 16:33:26 +0000 2018"
        }]
    }))
    .unwrap();
    let sig = compute_sig(TEST_SECRET, &body);

    let mut sub = fixture.nats.subscribe("twitter.favorite_events").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-twitter-webhooks-signature", &sig)
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

    assert_eq!(msg.subject.as_str(), "twitter.favorite_events");
}

/// Missing `x-twitter-webhooks-signature` → 401, no NATS publish.
#[tokio::test]
async fn missing_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "data": { "event_type": "profile.update.bio" }
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("twitter.>").await.unwrap();

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
    assert!(timed_out, "expected no NATS message on missing signature");
}

/// Wrong signature → 401, no NATS publish.
#[tokio::test]
async fn wrong_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "data": { "event_type": "profile.update.bio" }
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("twitter.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-twitter-webhooks-signature", "sha256=deadbeef")
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

/// Unroutable payload publishes to `twitter.unroutable`.
#[tokio::test]
async fn unknown_payload_publishes_to_unroutable_and_returns_200() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({ "hello": "world" })).unwrap();
    let sig = compute_sig(TEST_SECRET, &body);

    let mut sub = fixture.nats.subscribe("twitter.unroutable").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-twitter-webhooks-signature", &sig)
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

    assert_eq!(msg.subject.as_str(), "twitter.unroutable");
}
