//! Integration tests — real NATS JetStream + real HTTP router.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//! JetStream `publish_with_headers` also delivers to plain core-NATS subscribers,
//! so assertions use `nats.subscribe()` — no JetStream consumer setup needed.
//!
//! Run with:
//!   cargo test -p trogon-source-telegram --test webhook_e2e

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
use trogon_source_telegram::config::{TelegramSourceConfig, TelegramWebhookSecret};
use trogon_source_telegram::{provision, router};
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

const TEST_SECRET: &str = "tg-int-test-secret";

fn test_config() -> TelegramSourceConfig {
    TelegramSourceConfig {
        webhook_secret: TelegramWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("telegram").unwrap(),
        stream_name: NatsToken::new("TELEGRAM").unwrap(),
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

/// Happy path: valid token + `message` update → published to `telegram.message`.
#[tokio::test]
async fn valid_message_update_publishes_to_jetstream_and_returns_200() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "update_id": 123456789,
        "message": {
            "message_id": 42,
            "text": "hello world"
        }
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("telegram.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-telegram-bot-api-secret-token", TEST_SECRET)
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

    assert_eq!(msg.subject.as_str(), "telegram.message");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// Missing secret token header → 401, no NATS publish.
#[tokio::test]
async fn missing_token_returns_401_no_nats_message() {
    let fixture = setup().await;

    let mut sub = fixture.nats.subscribe("telegram.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({"update_id": 1, "message": {}}))
                        .unwrap(),
                ))
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

    let body = serde_json::to_vec(&serde_json::json!({"update_id": 1, "message": {}})).unwrap();

    let mut sub = fixture.nats.subscribe("telegram.>").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-telegram-bot-api-secret-token", "wrong-token")
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

/// Unknown update type → routed to `telegram.unroutable`.
#[tokio::test]
async fn unknown_update_type_routes_to_unroutable() {
    let fixture = setup().await;

    // Valid JSON with update_id but no known update type field
    let body = serde_json::to_vec(&serde_json::json!({
        "update_id": 999,
        "unknown_future_type": { "data": "something" }
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("telegram.unroutable").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-telegram-bot-api-secret-token", TEST_SECRET)
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for unroutable NATS message")
        .expect("subscription closed");

    assert_eq!(msg.subject.as_str(), "telegram.unroutable");
    assert_eq!(msg.payload.as_ref(), body.as_slice());
}

/// `callback_query` update → published to `telegram.callback_query`.
#[tokio::test]
async fn callback_query_update_routes_to_correct_subject() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "update_id": 777,
        "callback_query": {
            "id": "cq-id-1",
            "data": "button_clicked"
        }
    }))
    .unwrap();

    let mut sub = fixture
        .nats
        .subscribe("telegram.callback_query")
        .await
        .unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-telegram-bot-api-secret-token", TEST_SECRET)
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    assert_eq!(msg.subject.as_str(), "telegram.callback_query");
}

/// `update_id` is forwarded as `Nats-Msg-Id` and `X-Telegram-Update-Id` for deduplication.
#[tokio::test]
async fn update_id_is_set_as_nats_msg_id() {
    let fixture = setup().await;

    let body = serde_json::to_vec(&serde_json::json!({
        "update_id": 42000001,
        "message": { "message_id": 1, "text": "hi" }
    }))
    .unwrap();

    let mut sub = fixture.nats.subscribe("telegram.message").await.unwrap();

    let resp = fixture
        .app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook")
                .header("x-telegram-bot-api-secret-token", TEST_SECRET)
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
    assert_eq!(msg_id, Some("42000001"));

    let update_id_header = msg
        .headers
        .as_ref()
        .and_then(|h| h.get("X-Telegram-Update-Id"))
        .map(|v| v.as_str());
    assert_eq!(update_id_header, Some("42000001"));
}
