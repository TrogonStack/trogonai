//! Integration tests for the Linear webhook server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server with JetStream).
//!
//! JetStream `publish_with_headers` also delivers messages to plain core NATS
//! subscribers, so tests use `nats.subscribe(subject)` rather than JetStream
//! consumers — simpler and sufficient for verifying publish behaviour.
//!
//! Run with:
//!   cargo test -p trogon-linear --test webhook_e2e

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use futures_util::StreamExt as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use trogon_linear::{LinearConfig, serve};
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

static PORT_COUNTER: AtomicU16 = AtomicU16::new(29000);

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

/// Compute a Linear-style HMAC-SHA256 hex signature (no prefix).
fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn make_config(nats_port: u16, http_port: u16, secret: Option<&str>) -> LinearConfig {
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    if let Some(s) = secret {
        env.set("LINEAR_WEBHOOK_SECRET", s);
    }
    LinearConfig::from_env(&env)
}

async fn spawn_server(
    nats_port: u16,
    http_port: u16,
    secret: Option<&str>,
) -> async_nats::Client {
    let config = make_config(nats_port, http_port, secret);
    let nats_for_server = nats_client(nats_port).await;

    tokio::spawn(async move {
        serve(config, nats_for_server).await.expect("server error");
    });

    wait_for_port(http_port, Duration::from_secs(5)).await;
    nats_client(nats_port).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Happy path: a valid Issue create event with a correct HMAC signature is
/// accepted, returns 200 OK, and the body is published to `linear.Issue.create`.
#[tokio::test]
async fn webhook_issue_create_returns_200_and_is_published_to_nats() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "test-secret";
    let body = br#"{"type":"Issue","action":"create","data":{"id":"abc"},"webhookId":"wh-001"}"#;

    let nats = spawn_server(nats_port, http_port, Some(secret)).await;
    let mut sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    let sig = compute_sig(secret, body);
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("linear-signature", sig)
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

    let mut sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("linear-signature", "deadbeef")
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 401, "expected 401; got {}", resp.status());

    let result = tokio::time::timeout(Duration::from_millis(200), sub.next()).await;
    assert!(result.is_err(), "must not publish to NATS on invalid signature");
}

/// When a webhook secret is configured but the `linear-signature` header is
/// absent, the server must reject the request with 401 Unauthorized.
#[tokio::test]
async fn webhook_missing_signature_header_returns_401() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, Some("secret")).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        401,
        "expected 401 when signature header absent; got {}",
        resp.status()
    );
}

/// A payload missing the `type` field must be rejected with 400 Bad Request.
#[tokio::test]
async fn webhook_missing_type_field_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "expected 400 when 'type' absent; got {}", resp.status());
}

/// A payload missing the `action` field must be rejected with 400 Bad Request.
#[tokio::test]
async fn webhook_missing_action_field_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "expected 400 when 'action' absent; got {}", resp.status());
}

/// A non-JSON body must be rejected with 400 Bad Request.
#[tokio::test]
async fn webhook_non_json_body_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "text/plain")
        .body("this is not json")
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "expected 400 for non-JSON body; got {}", resp.status());
}

/// When no webhook secret is configured, signature validation is skipped
/// entirely. Any request with valid JSON containing `type` and `action` is accepted.
#[tokio::test]
async fn webhook_no_secret_configured_accepts_any_request() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let nats = spawn_server(nats_port, http_port, None).await;

    let mut sub = nats.subscribe("linear.Comment.create").await.expect("subscribe failed");

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        // No signature header at all — must still succeed when secret is not configured.
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Comment","action":"create","data":{"body":"hello"}}"#)
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

/// The NATS subject is `{prefix}.{type}.{action}` — an `Issue.update` event
/// must land on `linear.Issue.update`, not `linear.Issue.create`.
#[tokio::test]
async fn webhook_different_event_types_use_distinct_nats_subjects() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut create_sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");
    let mut update_sub = nats.subscribe("linear.Comment.update").await.expect("subscribe failed");

    let create_body = br#"{"type":"Issue","action":"create","data":{"id":"i-1"}}"#;
    let update_body = br#"{"type":"Comment","action":"update","data":{"id":"c-1"}}"#;

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(create_body.as_ref())
        .send()
        .await
        .unwrap();

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(update_body.as_ref())
        .send()
        .await
        .unwrap();

    let create_msg = tokio::time::timeout(Duration::from_secs(5), create_sub.next())
        .await
        .expect("timed out waiting for create message")
        .expect("subscriber closed");
    assert_eq!(create_msg.subject.as_str(), "linear.Issue.create");
    assert_eq!(create_msg.payload.as_ref(), create_body.as_ref());

    let update_msg = tokio::time::timeout(Duration::from_secs(5), update_sub.next())
        .await
        .expect("timed out waiting for update message")
        .expect("subscriber closed");
    assert_eq!(update_msg.subject.as_str(), "linear.Comment.update");
    assert_eq!(update_msg.payload.as_ref(), update_body.as_ref());
}

/// `X-Linear-Type`, `X-Linear-Action`, and `X-Linear-Webhook-Id` must be
/// forwarded as NATS message headers so consumers can filter without parsing the body.
#[tokio::test]
async fn webhook_nats_headers_forwarded() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"type":"Project","action":"update","data":{},"webhookId":"wh-xyz-999"}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("linear.Project.update").await.expect("subscribe failed");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let headers = msg.headers.expect("NATS message must have headers");

    assert_eq!(
        headers.get("X-Linear-Type").map(|v| v.as_str()).unwrap_or(""),
        "Project",
        "X-Linear-Type must be forwarded"
    );
    assert_eq!(
        headers.get("X-Linear-Action").map(|v| v.as_str()).unwrap_or(""),
        "update",
        "X-Linear-Action must be forwarded"
    );
    assert_eq!(
        headers.get("X-Linear-Webhook-Id").map(|v| v.as_str()).unwrap_or(""),
        "wh-xyz-999",
        "X-Linear-Webhook-Id must be forwarded"
    );
}

/// When `webhookId` is absent from the payload, `X-Linear-Webhook-Id` must
/// default to `"unknown"` in the NATS headers.
#[tokio::test]
async fn webhook_missing_webhook_id_defaults_to_unknown() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"type":"Cycle","action":"create","data":{}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("linear.Cycle.create").await.expect("subscribe failed");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let headers = msg.headers.expect("NATS message must have headers");
    let got_id = headers.get("X-Linear-Webhook-Id").map(|v| v.as_str()).unwrap_or("");
    assert_eq!(got_id, "unknown", "missing webhookId must default to 'unknown'; got: {got_id:?}");
}

/// Only `POST /webhook` is handled. A `GET` to the same path must return
/// 405 Method Not Allowed.
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
    let body =
        r#"{"type":"Issue","action":"create","data":{"title":"héllo wörld 🚀"}}"#.as_bytes();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
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

/// Custom `LINEAR_SUBJECT_PREFIX` must change the NATS subject used for publishing.
#[tokio::test]
async fn webhook_custom_subject_prefix_changes_nats_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    env.set("LINEAR_SUBJECT_PREFIX", "mylinear");
    let config = LinearConfig::from_env(&env);

    let nats_for_server = nats_client(nats_port).await;
    tokio::spawn(async move {
        serve(config, nats_for_server).await.expect("server error");
    });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    let nats = nats_client(nats_port).await;
    let mut sub = nats.subscribe("mylinear.Issue.create").await.expect("subscribe failed");
    let mut default_sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    let body = br#"{"type":"Issue","action":"create","data":{}}"#;
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for mylinear.Issue.create")
        .expect("subscriber closed");
    assert_eq!(msg.subject.as_str(), "mylinear.Issue.create");

    let stray = tokio::time::timeout(Duration::from_millis(200), default_sub.next()).await;
    assert!(stray.is_err(), "must not publish to linear.Issue.create when prefix is mylinear");
}

/// A large payload (~100 KB) must be forwarded to NATS unchanged.
#[tokio::test]
async fn webhook_large_payload_is_forwarded() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    let large_value = "x".repeat(100_000);
    let body = format!(r#"{{"type":"Issue","action":"create","data":{{"description":"{large_value}"}}}}"#);

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.clone())
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscriber closed");

    assert_eq!(
        msg.payload.as_ref(),
        body.as_bytes(),
        "large payload must be forwarded byte-for-byte"
    );
}

/// Ten webhooks sent concurrently must all arrive on NATS — no message is dropped.
#[tokio::test]
async fn webhook_concurrent_requests_all_published() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http_port}/webhook");

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let client = client.clone();
            let url = url.clone();
            tokio::spawn(async move {
                client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .body(format!(r#"{{"type":"Issue","action":"create","data":{{"i":{i}}}}}"#))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect();

    for handle in handles {
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    let mut received = 0usize;
    for _ in 0..10 {
        tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .expect("timed out waiting for NATS message")
            .expect("subscriber closed");
        received += 1;
    }
    assert_eq!(received, 10, "all 10 concurrent events must arrive on NATS");
}

/// A wildcard NATS subscription `linear.>` must receive events of any type.
#[tokio::test]
async fn webhook_wildcard_subscription_receives_all_event_types() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("linear.>").await.expect("subscribe failed");

    let events = [
        ("Issue", "create"),
        ("Issue", "update"),
        ("Comment", "create"),
        ("Project", "update"),
        ("Cycle", "remove"),
    ];

    for (event_type, action) in &events {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/webhook"))
            .header("Content-Type", "application/json")
            .body(format!(
                r#"{{"type":"{event_type}","action":"{action}","data":{{}}}}"#
            ))
            .send()
            .await
            .unwrap();
    }

    let mut received_subjects = std::collections::HashSet::new();
    for _ in 0..events.len() {
        let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .expect("timed out waiting for NATS message")
            .expect("subscriber closed");
        received_subjects.insert(msg.subject.to_string());
    }

    for (event_type, action) in &events {
        assert!(
            received_subjects.contains(&format!("linear.{event_type}.{action}")),
            "expected linear.{event_type}.{action} in received subjects; got: {received_subjects:?}"
        );
    }
}

// ── Health endpoint ───────────────────────────────────────────────────────────

/// `GET /health` must return 200 OK.
#[tokio::test]
async fn health_endpoint_returns_200() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port}/health"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200, "expected 200 from /health; got {}", resp.status());
}

/// `POST /health` is not registered — axum returns 405 Method Not Allowed.
#[tokio::test]
async fn health_endpoint_only_accepts_get() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/health"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 405, "expected 405 for POST /health; got {}", resp.status());
}

/// A request to an unknown path must return 404 Not Found.
#[tokio::test]
async fn unknown_path_returns_404() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port}/unknown"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 404, "expected 404 for unknown path; got {}", resp.status());
}

// ── Stream management ─────────────────────────────────────────────────────────

/// When a second server starts against a NATS that already has the `LINEAR`
/// stream, `get_or_create_stream` must succeed and the server must work normally.
#[tokio::test]
async fn server_starts_normally_when_stream_already_exists() {
    let (_container, nats_port) = start_nats().await;

    let http_port1 = next_port();
    spawn_server(nats_port, http_port1, None).await;

    let http_port2 = next_port();
    let nats = spawn_server(nats_port, http_port2, None).await;
    let mut sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    let body = br#"{"type":"Issue","action":"create","data":{}}"#;
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port2}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "second server instance must publish normally");

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");
    assert_eq!(msg.payload.as_ref(), body.as_ref());
}

/// When the JetStream stream is deleted while the server is running, the
/// server self-heals: it detects the ack failure, re-creates the stream, and
/// retries the publish — returning 200 without requiring a server restart.
#[tokio::test]
async fn webhook_recovers_when_jetstream_stream_is_gone() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    spawn_server(nats_port, http_port, None).await;

    let admin = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(admin);
    js.delete_stream("LINEAR").await.expect("failed to delete LINEAR stream");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        200,
        "server must self-heal after stream deletion; got {}",
        resp.status()
    );
}

/// When NATS is started without `--jetstream`, `ensure_stream` fails and
/// `serve()` must return `Err` immediately.
#[tokio::test]
async fn server_fails_to_start_without_jetstream() {
    let container: ContainerAsync<Nats> = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container");
    let nats_port = container.get_host_port_ipv4(4222).await.unwrap();

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", next_port().to_string());
    let config = LinearConfig::from_env(&env);

    let nats = nats_client(nats_port).await;
    let result = serve(config, nats).await;

    assert!(
        result.is_err(),
        "serve() must return Err when JetStream is not enabled on the NATS server"
    );
}

// ── Graceful shutdown ─────────────────────────────────────────────────────────

/// Sending SIGTERM to the server process must cause it to exit cleanly.
#[tokio::test]
async fn server_exits_cleanly_on_sigterm() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let bin = env!("CARGO_BIN_EXE_trogon-linear");
    let mut child = tokio::process::Command::new(bin)
        .env("NATS_URL", format!("localhost:{nats_port}"))
        .env("LINEAR_WEBHOOK_PORT", http_port.to_string())
        .env("RUST_LOG", "error")
        .spawn()
        .expect("failed to spawn trogon-linear binary");

    wait_for_port(http_port, Duration::from_secs(10)).await;

    let pid = child.id().expect("child has no pid");
    std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill failed");

    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("server did not exit within 10s after SIGTERM")
        .expect("wait failed");

    assert!(
        status.success(),
        "server must exit with code 0 after SIGTERM; got: {status:?}"
    );
}

/// SIGINT (Ctrl+C) must also trigger graceful shutdown.
#[tokio::test]
async fn server_exits_cleanly_on_sigint() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let bin = env!("CARGO_BIN_EXE_trogon-linear");
    let mut child = tokio::process::Command::new(bin)
        .env("NATS_URL", format!("localhost:{nats_port}"))
        .env("LINEAR_WEBHOOK_PORT", http_port.to_string())
        .env("RUST_LOG", "error")
        .spawn()
        .expect("failed to spawn trogon-linear binary");

    wait_for_port(http_port, Duration::from_secs(10)).await;

    let pid = child.id().expect("child has no pid");
    std::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .expect("kill -INT failed");

    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("server did not exit within 10s after SIGINT")
        .expect("wait failed");

    assert!(
        status.success(),
        "server must exit with code 0 after SIGINT; got: {status:?}"
    );
}

/// When NATS is unreachable at startup, the binary must exit with a non-zero
/// exit code within the connect timeout — it must not hang forever.
#[tokio::test]
async fn binary_exits_with_error_when_nats_unreachable() {
    let bin = env!("CARGO_BIN_EXE_trogon-linear");
    let http_port = next_port();

    let mut child = tokio::process::Command::new(bin)
        .env("NATS_URL", "localhost:19999")
        .env("LINEAR_WEBHOOK_PORT", http_port.to_string())
        .env("RUST_LOG", "error")
        .spawn()
        .expect("failed to spawn trogon-linear binary");

    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .expect("binary did not exit within 15s — it is hanging on NATS connect")
        .expect("wait failed");

    assert!(
        !status.success(),
        "binary must exit with non-zero code when NATS is unreachable; got: {status:?}"
    );
}

// ── Additional helpers ────────────────────────────────────────────────────────

/// Starts a server with a specific timestamp tolerance (in seconds).
async fn spawn_server_with_tolerance(
    nats_port: u16,
    http_port: u16,
    tolerance_secs: u64,
) -> async_nats::Client {
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", tolerance_secs.to_string());
    let config = LinearConfig::from_env(&env);
    let nats_for_server = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config, nats_for_server).await.expect("server error") });
    wait_for_port(http_port, Duration::from_secs(5)).await;
    nats_client(nats_port).await
}

/// After a full NATS server restart the JetStream stream is lost (NATS stores
/// it in memory by default in the test image).  async-nats reconnects at the
/// TCP level, but every subsequent `publish_with_headers` returns an error
/// When the JetStream stream is deleted while the server is running (which
/// models what happens when NATS restarts with ephemeral in-memory storage),
/// the server self-heals: on the first request after the loss it detects the
/// ack failure, re-creates the stream via `get_or_create_stream`, and retries
/// the publish — returning 200 without requiring a server restart.
///
/// Note: a testcontainers container.stop()/start() cycle reassigns ephemeral
/// host ports, so the existing NATS client cannot reconnect.  Deleting the
/// stream directly is the reliable way to exercise this exact code path.
#[tokio::test]
async fn webhook_recovers_after_stream_deletion() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let nats = spawn_server(nats_port, http_port, None).await;
    let js = async_nats::jetstream::new(nats);

    // 1. Normal operation — must succeed.
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await
        .expect("first request failed");
    assert_eq!(resp.status(), 200, "expected 200 before stream deletion; got {}", resp.status());

    // 2. Delete the stream to simulate the loss that occurs after a NATS restart
    //    with ephemeral (in-memory) JetStream storage.
    js.delete_stream("LINEAR").await.expect("failed to delete stream");

    // 3. Server detects the ack failure, re-creates the stream, retries — returns 200.
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await
        .expect("second request failed");
    assert_eq!(
        resp.status(),
        200,
        "server must self-heal after stream deletion; got {}",
        resp.status()
    );
}

/// When NATS is permanently gone, the server must eventually return 500 rather
/// than hanging.  The recovery path (ack timeout + ensure_stream timeout) adds
/// latency so we allow up to 25 s for the response.
#[tokio::test]
async fn webhook_returns_error_when_nats_connection_lost() {
    let (container, nats_port) = start_nats().await;
    let http_port = next_port();

    spawn_server(nats_port, http_port, None).await;

    drop(container);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await;

    match resp {
        Ok(r) => assert_eq!(
            r.status(),
            500,
            "server must return 500 when NATS is unreachable; got {}",
            r.status()
        ),
        Err(e) if e.is_timeout() => {
            panic!("server hung waiting for NATS — the HTTP handler must not block indefinitely: {e}")
        }
        Err(e) => panic!("unexpected HTTP error: {e}"),
    }
}

// ── Replay attack protection ──────────────────────────────────────────────────

/// A webhook with a current `webhookTimestamp` must be accepted.
#[tokio::test]
async fn webhook_current_timestamp_accepted() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    // Use a 60-second tolerance (the default).
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "60");
    let config = LinearConfig::from_env(&env);
    let nats_for_server = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config, nats_for_server).await.expect("server error") });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let body = format!(
        r#"{{"type":"Issue","action":"create","data":{{}},"webhookTimestamp":{now_ms}}}"#
    );

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200, "current timestamp must be accepted; got {}", resp.status());
}

/// A webhook with a `webhookTimestamp` older than the tolerance must be
/// rejected with 400 Bad Request to prevent replay attacks.
#[tokio::test]
async fn webhook_stale_timestamp_rejected_with_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    // Use a 5-second tolerance so we can reliably send a stale timestamp.
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "5");
    let config = LinearConfig::from_env(&env);
    let nats_for_server = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config, nats_for_server).await.expect("server error") });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    // Timestamp from 60 seconds ago — well outside the 5-second window.
    let stale_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        - 60_000;

    let body = format!(
        r#"{{"type":"Issue","action":"create","data":{{}},"webhookTimestamp":{stale_ms}}}"#
    );

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        400,
        "stale timestamp must be rejected with 400; got {}",
        resp.status()
    );
}

/// When `webhookTimestamp` is absent, the replay check is skipped and the
/// request is accepted normally.
#[tokio::test]
async fn webhook_missing_timestamp_skips_replay_check() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "60");
    let config = LinearConfig::from_env(&env);
    let nats_for_server = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config, nats_for_server).await.expect("server error") });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    // No webhookTimestamp field.
    let body = r#"{"type":"Issue","action":"create","data":{}}"#;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        200,
        "missing webhookTimestamp must not cause rejection; got {}",
        resp.status()
    );
}

/// When `LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS=0`, the replay check is
/// disabled entirely and even a stale timestamp is accepted.
#[tokio::test]
async fn webhook_timestamp_check_disabled_accepts_stale() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "0");
    let config = LinearConfig::from_env(&env);
    let nats_for_server = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config, nats_for_server).await.expect("server error") });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    // Timestamp from an hour ago.
    let old_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        - 3_600_000;

    let body = format!(
        r#"{{"type":"Issue","action":"create","data":{{}},"webhookTimestamp":{old_ms}}}"#
    );

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        200,
        "stale timestamp must be accepted when check is disabled; got {}",
        resp.status()
    );
}

// ── Timestamp edge cases ──────────────────────────────────────────────────────

/// A `webhookTimestamp` in the future must be accepted.
/// The code uses `saturating_sub(ts_ms)` which returns 0 when ts_ms > now_ms,
/// and `0 > tolerance_ms` is false — so future timestamps always pass.
#[tokio::test]
async fn webhook_future_timestamp_accepted() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server_with_tolerance(nats_port, http_port, 5).await;

    let future_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 3_600_000; // 1 hour in the future

    let body =
        format!(r#"{{"type":"Issue","action":"create","data":{{}},"webhookTimestamp":{future_ms}}}"#);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200, "future timestamp must be accepted; got {}", resp.status());
}

/// When `webhookTimestamp` is a JSON float, `serde_json::Value::as_u64()` returns None,
/// the replay check is skipped entirely, and the request is accepted regardless of age.
#[tokio::test]
async fn webhook_float_timestamp_skips_replay_check() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server_with_tolerance(nats_port, http_port, 5).await;

    // Float value from long ago — would be rejected if parsed as integer ms,
    // but as_u64() returns None for floats → replay check is skipped → 200.
    let body = r#"{"type":"Issue","action":"create","data":{},"webhookTimestamp":1234567890.5}"#;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        200,
        "float timestamp must skip replay check and be accepted; got {}",
        resp.status()
    );
}

/// When `webhookTimestamp` is a JSON string, `serde_json::Value::as_u64()` returns None,
/// the replay check is skipped, and the request is accepted regardless of age.
#[tokio::test]
async fn webhook_string_timestamp_skips_replay_check() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server_with_tolerance(nats_port, http_port, 5).await;

    // String timestamp from long ago — as_u64() returns None for strings → check skipped.
    let body =
        r#"{"type":"Issue","action":"create","data":{},"webhookTimestamp":"1234567890000"}"#;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        200,
        "string timestamp must skip replay check and be accepted; got {}",
        resp.status()
    );
}

// ── Type / action field type mismatches ───────────────────────────────────────

/// When `type` is a JSON number instead of a string, `as_str()` returns None → 400.
#[tokio::test]
async fn webhook_type_as_number_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":123,"action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "numeric 'type' must be rejected with 400; got {}", resp.status());
}

/// When `action` is a JSON boolean instead of a string, `as_str()` returns None → 400.
#[tokio::test]
async fn webhook_action_as_boolean_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":true,"data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        400,
        "boolean 'action' must be rejected with 400; got {}",
        resp.status()
    );
}

/// When `webhookId` is a JSON number (not a string), `as_str()` returns None
/// and `X-Linear-Webhook-Id` must default to "unknown".
#[tokio::test]
async fn webhook_webhook_id_as_number_defaults_to_unknown() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"type":"Issue","action":"create","data":{},"webhookId":42}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let headers = msg.headers.expect("NATS message must have headers");
    let webhook_id = headers.get("X-Linear-Webhook-Id").map(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        webhook_id, "unknown",
        "numeric webhookId must default to 'unknown'; got: {webhook_id:?}"
    );
}

// ── Non-object JSON body ───────────────────────────────────────────────────────

/// A JSON `null` body is valid JSON but `parsed.get("type")` returns None → 400.
#[tokio::test]
async fn webhook_json_null_body_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body("null")
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "JSON null body must return 400; got {}", resp.status());
}

/// A JSON array body is valid JSON but `parsed.get("type")` returns None → 400.
#[tokio::test]
async fn webhook_json_array_body_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"[{"type":"Issue","action":"create"}]"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "JSON array body must return 400; got {}", resp.status());
}

/// A JSON string body is valid JSON but `parsed.get("type")` returns None → 400.
#[tokio::test]
async fn webhook_json_string_body_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#""Issue""#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "JSON string body must return 400; got {}", resp.status());
}

// ── Signature edge cases ───────────────────────────────────────────────────────

/// An empty `linear-signature` header value fails HMAC hex verification → 401.
/// `hex::decode("")` returns Ok(vec![]), but verify_slice(&[]) fails since
/// the expected HMAC digest is 32 bytes.
#[tokio::test]
async fn webhook_empty_signature_returns_401() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, Some("some-secret")).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("linear-signature", "")
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        401,
        "empty signature must be rejected with 401; got {}",
        resp.status()
    );
}

// ── Stream configuration ───────────────────────────────────────────────────────

/// When `LINEAR_STREAM_NAME` is customised, the server must create a JetStream
/// stream with that name (not the default `LINEAR`) and publish events into it.
#[tokio::test]
async fn server_creates_stream_with_custom_name() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    env.set("LINEAR_STREAM_NAME", "MYLINEAR");
    let config = LinearConfig::from_env(&env);
    let nats_for_server = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config, nats_for_server).await.expect("server error") });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    let admin = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(admin);

    // Custom stream must exist.
    js.get_stream("MYLINEAR").await.expect("MYLINEAR stream must exist");

    // Default stream must NOT exist.
    let default_result = js.get_stream("LINEAR").await;
    assert!(
        default_result.is_err(),
        "LINEAR stream must NOT exist when a custom name is configured"
    );
}

/// When `LINEAR_STREAM_MAX_AGE_SECS` is customised, the JetStream stream must
/// be created with the specified `max_age` in its configuration.
#[tokio::test]
async fn server_creates_stream_with_custom_max_age() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", http_port.to_string());
    env.set("LINEAR_STREAM_MAX_AGE_SECS", "3600");
    env.set("LINEAR_STREAM_NAME", "LINEAR_AGE_TEST"); // isolated stream name
    let config = LinearConfig::from_env(&env);
    let nats_for_server = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config, nats_for_server).await.expect("server error") });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    let admin = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(admin);

    let stream = js.get_stream("LINEAR_AGE_TEST").await.expect("stream must exist");
    let info = stream.cached_info();

    assert_eq!(
        info.config.max_age,
        Duration::from_secs(3600),
        "stream max_age must match LINEAR_STREAM_MAX_AGE_SECS; got {:?}",
        info.config.max_age
    );
}

// ── Body edge cases ────────────────────────────────────────────────────────────

/// A completely empty request body is not valid JSON → 400.
#[tokio::test]
async fn webhook_empty_body_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body("")
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "empty body must return 400; got {}", resp.status());
}

// ── Signature header case sensitivity ─────────────────────────────────────────

/// HTTP headers are case-insensitive per RFC 7230. A `Linear-Signature` header
/// (capitalised) must be treated identically to `linear-signature`.
#[tokio::test]
async fn webhook_signature_header_case_insensitive() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "test-secret";
    let body = br#"{"type":"Issue","action":"create","data":{}}"#;

    spawn_server(nats_port, http_port, Some(secret)).await;

    let sig = compute_sig(secret, body);
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Linear-Signature", sig) // capitalised
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        200,
        "signature header must be accepted regardless of case; got {}",
        resp.status()
    );
}

// ── Timestamp additional edge cases ───────────────────────────────────────────

/// A negative `webhookTimestamp` (e.g. -1000) is a valid JSON number but
/// `as_u64()` returns None for negative values, so the replay check is
/// skipped and the request is accepted.
#[tokio::test]
async fn webhook_negative_timestamp_skips_replay_check() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server_with_tolerance(nats_port, http_port, 5).await;

    let body = r#"{"type":"Issue","action":"create","data":{},"webhookTimestamp":-1000}"#;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        200,
        "negative timestamp must skip replay check and be accepted; got {}",
        resp.status()
    );
}

// ── Empty type / action ────────────────────────────────────────────────────────

/// A `type` field that is an empty string must be rejected with 400.
/// Without validation the server would try to publish to `linear..create`
/// (empty NATS subject token), which is an invalid subject.
#[tokio::test]
async fn webhook_empty_type_field_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let body = r#"{"type":"","action":"create","data":{}}"#;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "empty 'type' must return 400; got {}", resp.status());
}

/// An `action` field that is an empty string must be rejected with 400.
/// Without validation the server would publish to `linear.Issue.`
/// (trailing-dot NATS subject), which is invalid.
#[tokio::test]
async fn webhook_empty_action_field_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let body = r#"{"type":"Issue","action":"","data":{}}"#;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "empty 'action' must return 400; got {}", resp.status());
}

// ── webhookId as JSON null ─────────────────────────────────────────────────────

/// A `webhookId` field with JSON `null` value must be treated the same as a
/// missing field: the header `X-Linear-Webhook-Id` is set to `"unknown"`.
#[tokio::test]
async fn webhook_webhook_id_as_null_defaults_to_unknown() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("linear.Issue.create").await.expect("subscribe failed");

    let body = r#"{"type":"Issue","action":"create","data":{},"webhookId":null}"#;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200, "null webhookId must be accepted; got {}", resp.status());

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscriber closed");

    let webhook_id = msg
        .headers
        .as_ref()
        .and_then(|h| h.get("X-Linear-Webhook-Id"))
        .map(|v| v.as_str());

    assert_eq!(webhook_id, Some("unknown"), "null webhookId must produce X-Linear-Webhook-Id: unknown");
}

// ── Additional HTTP methods on /webhook ───────────────────────────────────────

/// PUT /webhook must return 405 Method Not Allowed.
#[tokio::test]
async fn webhook_put_returns_405() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .put(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 405, "PUT /webhook must return 405; got {}", resp.status());
}

/// DELETE /webhook must return 405 Method Not Allowed.
#[tokio::test]
async fn webhook_delete_returns_405() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .delete(format!("http://127.0.0.1:{http_port}/webhook"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 405, "DELETE /webhook must return 405; got {}", resp.status());
}

/// PATCH /webhook must return 405 Method Not Allowed.
#[tokio::test]
async fn webhook_patch_returns_405() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .patch(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 405, "PATCH /webhook must return 405; got {}", resp.status());
}

// ── webhookTimestamp = 0 ──────────────────────────────────────────────────────

/// A `webhookTimestamp` of 0 (Unix epoch, Jan 1 1970) must be rejected as
/// a replay: its age is ~56 years, far beyond any reasonable tolerance.
#[tokio::test]
async fn webhook_zero_timestamp_rejected() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server_with_tolerance(nats_port, http_port, 60).await;

    let body = r#"{"type":"Issue","action":"create","data":{},"webhookTimestamp":0}"#;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        400,
        "webhookTimestamp=0 (epoch) must be rejected as stale; got {}",
        resp.status()
    );
}

/// A `webhookTimestamp` just inside the tolerance boundary must be accepted.
/// The check is `age_ms > tolerance_ms` (strict greater-than), so a timestamp
/// exactly one second before the boundary passes.
#[tokio::test]
async fn webhook_timestamp_just_inside_boundary_accepted() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    // Use a 30-second tolerance. Send a timestamp 29 seconds ago — safely inside.
    spawn_server_with_tolerance(nats_port, http_port, 30).await;

    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        - 29_000; // 29s ago, tolerance is 30s

    let body =
        format!(r#"{{"type":"Issue","action":"create","data":{{}},"webhookTimestamp":{ts_ms}}}"#);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        200,
        "timestamp just inside tolerance must be accepted; got {}",
        resp.status()
    );
}

// ── Invalid NATS subject characters in type / action ──────────────────────────

/// `type` containing a space would produce `linear.Issue Comment.create` —
/// an invalid NATS subject token. Must be rejected with 400.
#[tokio::test]
async fn webhook_type_with_space_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue Comment","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "space in 'type' must return 400; got {}", resp.status());
}

/// `action` containing a space must also be rejected with 400.
#[tokio::test]
async fn webhook_action_with_space_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"create now","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "space in 'action' must return 400; got {}", resp.status());
}

/// `type` containing `*` (NATS single-token wildcard) must be rejected with 400.
#[tokio::test]
async fn webhook_type_with_nats_wildcard_star_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue*","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "'*' in 'type' must return 400; got {}", resp.status());
}

/// `type` containing `>` (NATS full-wildcard) must be rejected with 400.
#[tokio::test]
async fn webhook_type_with_nats_wildcard_gt_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue>","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "'>' in 'type' must return 400; got {}", resp.status());
}

/// `type` containing `.` would silently add extra subject levels
/// (e.g. `linear.foo.bar.create`). Must be rejected with 400.
#[tokio::test]
async fn webhook_type_with_dot_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"foo.bar","action":"create","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "'.' in 'type' must return 400; got {}", resp.status());
}

/// `action` containing `*` (NATS wildcard) must be rejected with 400.
#[tokio::test]
async fn webhook_action_with_nats_wildcard_star_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"type":"Issue","action":"cre*ate","data":{}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "'*' in 'action' must return 400; got {}", resp.status());
}

/// `action` containing a control character (tab, `\t`) must be rejected with 400.
/// This exercises branch 2 (`b < 32`) of `has_invalid_nats_chars` for `action`.
#[tokio::test]
async fn webhook_action_with_control_char_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body("{\"type\":\"Issue\",\"action\":\"cre\\u0009ate\",\"data\":{}}")
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "control char in 'action' must return 400; got {}", resp.status());
}

/// `type` containing a null byte (U+0000) is valid JSON but produces a NATS
/// subject with a control character. Must be rejected with 400.
#[tokio::test]
async fn webhook_type_with_null_byte_returns_400() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    // JSON \u0000 is the null byte
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body("{\"type\":\"foo\\u0000bar\",\"action\":\"create\",\"data\":{}}")
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 400, "null byte in 'type' must return 400; got {}", resp.status());
}
