//! Integration tests for the GitHub webhook server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server with JetStream).
//!
//! JetStream `publish_with_headers` also delivers messages to plain core NATS
//! subscribers, so tests use `nats.subscribe(subject)` rather than JetStream
//! consumers — simpler and sufficient for verifying publish behaviour.
//!
//! Run with:
//!   cargo test -p trogon-github --test webhook_e2e

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use futures_util::StreamExt as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use trogon_github::{GithubConfig, serve};
use trogon_std::env::InMemoryEnv;

type HmacSha256 = Hmac<Sha256>;

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container: ContainerAsync<Nats> = Nats::default()
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
        .expect("Failed to connect to NATS")
}

static PORT_COUNTER: AtomicU16 = AtomicU16::new(28000);

fn next_port() -> u16 {
    PORT_COUNTER.fetch_add(1, Ordering::SeqCst)
}

async fn wait_for_port(port: u16, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await {
            Ok(_) => return,
            Err(_) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("Port {port} not ready within {timeout:?}");
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// Compute a `sha256=<hex>` HMAC-SHA256 signature as GitHub would produce it.
fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Build a `GithubConfig` from env-var style settings.
fn make_config(nats_port: u16, http_port: u16, secret: Option<&str>) -> GithubConfig {
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("GITHUB_WEBHOOK_PORT", http_port.to_string());
    if let Some(s) = secret {
        env.set("GITHUB_WEBHOOK_SECRET", s);
    }
    GithubConfig::from_env(&env)
}

/// Start the server in a background task; return a connected NATS client for assertions.
async fn spawn_server(
    nats_port: u16,
    http_port: u16,
    secret: Option<&str>,
) -> async_nats::Client {
    let config = make_config(nats_port, http_port, secret);
    let nats_for_server = nats_client(nats_port).await;

    tokio::spawn(async move {
        serve(config, nats_for_server)
            .await
            .expect("server error");
    });

    wait_for_port(http_port, Duration::from_secs(5)).await;
    nats_client(nats_port).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Happy path: a valid `push` event with a correct HMAC signature is accepted,
/// returns 200 OK, and the raw JSON body is published to NATS on `github.push`.
#[tokio::test]
async fn webhook_push_event_returns_200_and_is_published_to_nats() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "test-secret";
    let body = br#"{"ref":"refs/heads/main","repository":{"full_name":"org/repo"}}"#;

    let nats = spawn_server(nats_port, http_port, Some(secret)).await;

    // Subscribe via core NATS before the request so no messages are missed.
    let mut sub = nats.subscribe("github.push").await.expect("subscribe failed");

    let sig = compute_sig(secret, body);
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Hub-Signature-256", sig)
        .header("X-GitHub-Event", "push")
        .header("X-GitHub-Delivery", "abc-123")
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200, "expected 200 OK; got {}", resp.status());

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscriber closed");

    assert_eq!(msg.payload.as_ref(), body.as_ref(), "NATS payload must match request body");
}

/// A request with an incorrect HMAC signature is rejected with 401 Unauthorized.
/// The event must NOT be published to NATS.
#[tokio::test]
async fn webhook_invalid_signature_returns_401() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let nats = spawn_server(nats_port, http_port, Some("correct-secret")).await;

    let mut sub = nats.subscribe("github.push").await.expect("subscribe failed");

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Hub-Signature-256", "sha256=deadbeef")
        .header("X-GitHub-Event", "push")
        .header("Content-Type", "application/json")
        .body(r#"{"ref":"refs/heads/main"}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 401, "expected 401; got {}", resp.status());

    // No message should arrive.
    let result = tokio::time::timeout(Duration::from_millis(200), sub.next()).await;
    assert!(result.is_err(), "must not publish to NATS on invalid signature");
}

/// When a webhook secret is configured but the `X-Hub-Signature-256` header is
/// absent, the server must reject the request with 401 Unauthorized.
#[tokio::test]
async fn webhook_missing_signature_header_returns_401() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, Some("secret")).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-GitHub-Event", "push")
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 401, "expected 401 when signature header absent; got {}", resp.status());
}

/// A request without an `X-GitHub-Event` header must be rejected with 400 Bad Request.
/// Signature validation is irrelevant — the event type is required regardless.
#[tokio::test]
async fn webhook_missing_event_type_header_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "test-secret";
    let body = b"{}";
    spawn_server(nats_port, http_port, Some(secret)).await;

    let sig = compute_sig(secret, body);
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Hub-Signature-256", sig)
        // Deliberately omit X-GitHub-Event.
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "expected 400 when X-GitHub-Event absent; got {}", resp.status());
}

/// When no webhook secret is configured, signature validation is skipped
/// entirely.  Any request with `X-GitHub-Event` is accepted — no signature header required.
#[tokio::test]
async fn webhook_no_secret_configured_accepts_any_request() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let nats = spawn_server(nats_port, http_port, None).await;

    let mut sub = nats.subscribe("github.ping").await.expect("subscribe failed");

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        // No signature header at all — must still succeed when secret is not configured.
        .header("X-GitHub-Event", "ping")
        .header("Content-Type", "application/json")
        .body(r#"{"zen":"Keep it logically awesome."}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200, "expected 200 with no secret configured; got {}", resp.status());

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscriber closed");

    assert!(!msg.payload.is_empty(), "NATS payload must not be empty");
}

/// The NATS subject is `{prefix}.{event_type}` — for a `pull_request` event
/// the message must be published to `github.pull_request`, not `github.push`.
#[tokio::test]
async fn webhook_pull_request_event_uses_correct_nats_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "pr-secret";
    let body = br#"{"action":"opened","number":42}"#;

    let nats = spawn_server(nats_port, http_port, Some(secret)).await;

    // Subscribe to the pull_request subject specifically.
    let mut sub = nats
        .subscribe("github.pull_request")
        .await
        .expect("subscribe failed");

    let sig = compute_sig(secret, body);
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Hub-Signature-256", sig)
        .header("X-GitHub-Event", "pull_request")
        .header("X-GitHub-Delivery", "pr-delivery-001")
        .body(body.as_ref())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    assert_eq!(msg.subject.as_str(), "github.pull_request");
    assert_eq!(msg.payload.as_ref(), body.as_ref());
}

/// `X-GitHub-Delivery` must be forwarded as a NATS message header so consumers
/// can correlate NATS messages back to the original GitHub webhook delivery.
#[tokio::test]
async fn webhook_delivery_id_forwarded_in_nats_headers() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"action":"created"}"#;
    let delivery_id = "delivery-uuid-xyz-9999";

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("github.issues").await.expect("subscribe failed");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-GitHub-Event", "issues")
        .header("X-GitHub-Delivery", delivery_id)
        .body(body.as_ref())
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let headers = msg.headers.expect("NATS message must have headers");
    let got_delivery = headers
        .get("X-GitHub-Delivery")
        .map(|v| v.as_str())
        .unwrap_or("");

    assert_eq!(
        got_delivery, delivery_id,
        "X-GitHub-Delivery header must be forwarded to NATS; got: {got_delivery:?}"
    );
}

/// The `X-GitHub-Event` header value must be forwarded as a NATS message header
/// so consumers can filter by event type without inspecting the body.
#[tokio::test]
async fn webhook_event_type_forwarded_in_nats_headers() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"stars":42}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("github.star").await.expect("subscribe failed");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-GitHub-Event", "star")
        .header("X-GitHub-Delivery", "star-delivery-001")
        .body(body.as_ref())
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let headers = msg.headers.expect("NATS message must have headers");
    let got_event = headers
        .get("X-GitHub-Event")
        .map(|v| v.as_str())
        .unwrap_or("");

    assert_eq!(got_event, "star", "X-GitHub-Event must be forwarded to NATS");
}

/// Only `POST /webhook` is handled.  A `GET` to the same path must return
/// 405 Method Not Allowed (axum default for route method mismatch).
#[tokio::test]
async fn webhook_get_request_returns_405() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port}/webhook"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 405, "expected 405 Method Not Allowed for GET; got {}", resp.status());
}

/// The raw request body is forwarded byte-for-byte to NATS without any
/// re-serialization or transformation.
#[tokio::test]
async fn webhook_raw_body_preserved_in_nats_message() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    // Body with Unicode and special characters to stress-test byte transparency.
    let body = r#"{"message":"héllo wörld 🚀","nested":{"key":"value"}}"#.as_bytes();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("github.release").await.expect("subscribe failed");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-GitHub-Event", "release")
        .header("X-GitHub-Delivery", "release-123")
        .body(body)
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    assert_eq!(
        msg.payload.as_ref(),
        body,
        "NATS payload must be the raw body bytes unchanged"
    );
}

/// When no `X-GitHub-Delivery` header is present the server must still publish
/// the event — the delivery ID defaults to `"unknown"` in the NATS headers.
#[tokio::test]
async fn webhook_missing_delivery_id_defaults_to_unknown() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("github.workflow_run").await.expect("subscribe failed");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-GitHub-Event", "workflow_run")
        // Deliberately omit X-GitHub-Delivery.
        .body(r#"{"status":"completed"}"#)
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let headers = msg.headers.expect("NATS message must have headers");
    let got_delivery = headers
        .get("X-GitHub-Delivery")
        .map(|v| v.as_str())
        .unwrap_or("");

    assert_eq!(
        got_delivery, "unknown",
        "missing X-GitHub-Delivery must default to 'unknown' in NATS headers; got: {got_delivery:?}"
    );
}

/// Two different event types published through the same server must land on
/// their respective NATS subjects without cross-contamination.
#[tokio::test]
async fn webhook_different_event_types_use_distinct_nats_subjects() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut push_sub = nats.subscribe("github.push").await.expect("subscribe failed");
    let mut tag_sub = nats.subscribe("github.create").await.expect("subscribe failed");

    let push_body = br#"{"ref":"refs/heads/main"}"#;
    let tag_body = br#"{"ref":"v1.0.0","ref_type":"tag"}"#;

    // Send push event.
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-GitHub-Event", "push")
        .header("X-GitHub-Delivery", "push-del-1")
        .body(push_body.as_ref())
        .send()
        .await
        .unwrap();

    // Send create (tag) event.
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-GitHub-Event", "create")
        .header("X-GitHub-Delivery", "create-del-1")
        .body(tag_body.as_ref())
        .send()
        .await
        .unwrap();

    let push_msg = tokio::time::timeout(Duration::from_secs(5), push_sub.next())
        .await
        .expect("timed out waiting for push message")
        .expect("subscriber closed");
    assert_eq!(push_msg.payload.as_ref(), push_body.as_ref());
    assert_eq!(push_msg.subject.as_str(), "github.push");

    let tag_msg = tokio::time::timeout(Duration::from_secs(5), tag_sub.next())
        .await
        .expect("timed out waiting for create message")
        .expect("subscriber closed");
    assert_eq!(tag_msg.payload.as_ref(), tag_body.as_ref());
    assert_eq!(tag_msg.subject.as_str(), "github.create");
}
