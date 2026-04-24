//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-source-sentry --test webhook_e2e

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
use trogon_source_sentry::{SentryClientSecret, SentryConfig, provision, router};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

const TEST_SECRET: &str = "sentry-int-test-secret";

// ── Helpers ────────────────────────────────────────────────────────────────────

fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn test_config() -> SentryConfig {
    SentryConfig {
        client_secret: SentryClientSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("sentry").unwrap(),
        stream_name: NatsToken::new("SENTRY").unwrap(),
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

fn webhook_request(body: &[u8], resource: &str, timestamp: &str, request_id: &str, signature: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header("sentry-hook-resource", resource)
        .header("sentry-hook-timestamp", timestamp)
        .header("request-id", request_id);

    if let Some(sig) = signature {
        builder = builder.header("sentry-hook-signature", sig);
    }

    builder.body(Body::from(body.to_vec())).unwrap()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Happy path: valid issue webhook publishes to `sentry.issue.created`.
#[tokio::test]
async fn valid_issue_event_publishes_to_nats_and_returns_200() {
    let fixture = setup().await;

    let body = br#"{"action":"created","data":{"issue":{"id":"123"}}}"#;
    let sig = compute_sig(TEST_SECRET, body);

    let mut sub = fixture.nats.subscribe("sentry.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-int-1", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscription closed");

    assert_eq!(msg.subject.as_str(), "sentry.issue.created");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// Missing `Sentry-Hook-Signature` → 401, no NATS publish.
#[tokio::test]
async fn missing_signature_returns_401_no_nats_message() {
    let fixture = setup().await;

    let body = br#"{"action":"created"}"#;

    let mut sub = fixture.nats.subscribe("sentry.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-int-2", None))
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

    let body = br#"{"action":"created"}"#;

    let mut sub = fixture.nats.subscribe("sentry.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(webhook_request(
            body,
            "issue",
            "1711315768",
            "req-int-3",
            Some("deadbeefdeadbeef"),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "expected no NATS message on wrong signature");
}

/// Missing `Sentry-Hook-Resource` → 400.
#[tokio::test]
async fn missing_resource_returns_400() {
    let fixture = setup().await;

    let body = br#"{"action":"created"}"#;
    let sig = compute_sig(TEST_SECRET, body);

    let request = Request::builder()
        .method("POST")
        .uri("/webhook")
        .header("sentry-hook-signature", &sig)
        .header("sentry-hook-timestamp", "1711315768")
        .header("request-id", "req-int-4")
        .body(Body::from(body.to_vec()))
        .unwrap();

    let resp = fixture.app.oneshot(request).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// `request-id` forwarded as `Nats-Msg-Id` for deduplication.
#[tokio::test]
async fn request_id_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let body = br#"{"action":"resolved","data":{"issue":{"id":"456"}}}"#;
    let sig = compute_sig(TEST_SECRET, body);

    let mut sub = fixture.nats.subscribe("sentry.issue.resolved").await.unwrap();

    let resp = fixture
        .app
        .oneshot(webhook_request(body, "issue", "1711315768", "req-dedup-999", Some(&sig)))
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
    assert_eq!(msg_id, Some("req-dedup-999"));
}

/// Event alert webhook publishes to `sentry.event_alert.<action>`.
#[tokio::test]
async fn event_alert_publishes_to_correct_subject() {
    let fixture = setup().await;

    let body = br#"{"action":"triggered","data":{"event":{}}}"#;
    let sig = compute_sig(TEST_SECRET, body);

    let mut sub = fixture.nats.subscribe("sentry.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(webhook_request(body, "event_alert", "1711315768", "req-int-5", Some(&sig)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    assert_eq!(msg.subject.as_str(), "sentry.event_alert.triggered");
}
