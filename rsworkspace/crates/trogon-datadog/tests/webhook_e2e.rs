//! Integration tests for the Datadog webhook server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-datadog --test webhook_e2e

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use futures_util::StreamExt as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_datadog::{DatadogConfig, serve};
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

/// Compute a Datadog-style HMAC-SHA256 signature (hex, no prefix).
fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn make_config(nats_port: u16, http_port: u16, secret: Option<&str>) -> DatadogConfig {
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("DATADOG_WEBHOOK_PORT", http_port.to_string());
    if let Some(s) = secret {
        env.set("DATADOG_WEBHOOK_SECRET", s);
    }
    DatadogConfig::from_env(&env)
}

async fn spawn_server(nats_port: u16, http_port: u16, secret: Option<&str>) -> async_nats::Client {
    let config = make_config(nats_port, http_port, secret);
    let nats_for_server = nats_client(nats_port).await;

    tokio::spawn(async move {
        serve(config, nats_for_server).await.expect("server error");
    });

    wait_for_port(http_port, Duration::from_secs(5)).await;
    nats_client(nats_port).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Happy path: a triggered alert with a correct signature returns 200 and
/// is published on `datadog.alert`.
#[tokio::test]
async fn webhook_alert_triggered_returns_200_and_published_to_nats() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "test-secret";
    let body = br#"{"alert_transition":"Triggered","monitor_name":"High error rate","title":"[Triggered] High error rate"}"#;

    let nats = spawn_server(nats_port, http_port, Some(secret)).await;
    let mut sub = nats.subscribe("datadog.alert").await.expect("subscribe");

    let sig = compute_sig(secret, body);
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Datadog-Signature", sig)
        .header("DD-Request-ID", "req-001")
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    assert_eq!(msg.payload.as_ref(), body.as_ref());
}

/// Recovered alerts are published on `datadog.alert.recovered`.
#[tokio::test]
async fn webhook_alert_recovered_publishes_to_correct_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"alert_transition":"Recovered","monitor_name":"High error rate"}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats
        .subscribe("datadog.alert.recovered")
        .await
        .expect("subscribe");

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    assert_eq!(msg.payload.as_ref(), body.as_ref());
}

/// Re-Triggered alerts are published on `datadog.alert` (same as Triggered).
#[tokio::test]
async fn webhook_alert_retriggered_publishes_to_alert_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"alert_transition":"Re-Triggered","monitor_name":"Disk full"}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("datadog.alert").await.expect("subscribe");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    assert_eq!(msg.payload.as_ref(), body.as_ref());
}

/// Unknown / missing alert_transition falls back to `datadog.event`.
#[tokio::test]
async fn webhook_unknown_transition_publishes_to_event_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"title":"Some Datadog notification"}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("datadog.event").await.expect("subscribe");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");
}

/// An invalid signature is rejected with 401.
#[tokio::test]
async fn webhook_invalid_signature_returns_401() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, Some("correct-secret")).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Datadog-Signature", "deadbeef")
        .header("Content-Type", "application/json")
        .body(r#"{"alert_transition":"Triggered"}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 401);
}

/// A missing signature header is rejected with 401 when a secret is configured.
#[tokio::test]
async fn webhook_missing_signature_returns_401() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, Some("secret")).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"alert_transition":"Triggered"}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 401);
}

/// When no secret is configured, any request (without signature) is accepted.
#[tokio::test]
async fn webhook_no_secret_accepts_unsigned_requests() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"alert_transition":"Triggered"}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);
}

/// Health endpoint always returns 200.
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

    assert_eq!(resp.status(), 200);
}

/// NATS headers are forwarded: DD-Request-ID and X-Datadog-Event-Type.
#[tokio::test]
async fn webhook_sets_nats_headers() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"alert_transition":"Triggered"}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("datadog.alert").await.expect("subscribe");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("DD-Request-ID", "req-abc-123")
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let headers = msg.headers.expect("no headers");
    assert_eq!(
        headers.get("X-Datadog-Event-Type").map(|v| v.as_str()),
        Some("alert")
    );
    assert_eq!(
        headers.get("DD-Request-ID").map(|v| v.as_str()),
        Some("req-abc-123")
    );
}

/// When the DATADOG JetStream stream is deleted, the server returns 500
/// (no self-healing, consistent with trogon-github behavior).
#[tokio::test]
async fn webhook_returns_500_when_jetstream_stream_is_gone() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;

    let js = async_nats::jetstream::new(nats);
    js.delete_stream("DATADOG")
        .await
        .expect("failed to delete DATADOG stream");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"alert_transition":"Triggered"}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 500);
}

/// Raw payload bytes are preserved exactly — no re-serialisation.
#[tokio::test]
async fn webhook_preserves_payload_bytes_exactly() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"alert_transition":"Triggered","monitor_name":"Disk full","value":99.5,"tags":["env:prod","service:api"]}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("datadog.alert").await.expect("subscribe");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    assert_eq!(
        msg.payload.as_ref(),
        body.as_ref(),
        "payload must be bit-for-bit identical"
    );
}

/// Non-JSON body is accepted and published on `datadog.event`.
#[tokio::test]
async fn webhook_non_json_body_publishes_to_event_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = b"plain text notification from datadog";

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("datadog.event").await.expect("subscribe");

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);

    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");
}

/// Missing DD-Request-ID header is gracefully handled — defaults to "unknown".
#[tokio::test]
async fn webhook_missing_request_id_header_still_returns_200() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"alert_transition":"Triggered"}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("datadog.alert").await.expect("subscribe");

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let headers = msg.headers.expect("no headers");
    assert_eq!(
        headers.get("DD-Request-ID").map(|v| v.as_str()),
        Some("unknown"),
        "missing DD-Request-ID must default to 'unknown'"
    );
}

/// GitHub-style `sha256=` prefixed signature is rejected (Datadog uses raw hex).
#[tokio::test]
async fn webhook_github_style_signature_prefix_is_rejected() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "test-secret";
    let body = br#"{"alert_transition":"Triggered"}"#;

    spawn_server(nats_port, http_port, Some(secret)).await;

    // Compute a valid HMAC but add the GitHub-style sha256= prefix — must fail.
    let sig = format!("sha256={}", compute_sig(secret, body));
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Datadog-Signature", sig)
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        401,
        "sha256= prefixed signature must be rejected"
    );
}

/// Custom subject prefix via DATADOG_SUBJECT_PREFIX env var.
#[tokio::test]
async fn webhook_custom_subject_prefix_is_used() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("DATADOG_WEBHOOK_PORT", http_port.to_string());
    env.set("DATADOG_SUBJECT_PREFIX", "dd");
    env.set("DATADOG_STREAM_NAME", "DD");
    let config = DatadogConfig::from_env(&env);

    let nats_for_server = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("connect");
    tokio::spawn(async move { serve(config, nats_for_server).await.ok() });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("connect");
    let mut sub = nats.subscribe("dd.alert").await.expect("subscribe");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"alert_transition":"Triggered"}"#)
        .send()
        .await
        .expect("HTTP request failed");

    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out on dd.alert — custom prefix not applied")
        .expect("subscriber closed");
}

/// Multiple sequential alerts are all published correctly.
#[tokio::test]
async fn webhook_multiple_alerts_published_in_order() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("datadog.>").await.expect("subscribe");

    let payloads = [
        r#"{"alert_transition":"Triggered","monitor_name":"A"}"#,
        r#"{"alert_transition":"Recovered","monitor_name":"A"}"#,
        r#"{"alert_transition":"Triggered","monitor_name":"B"}"#,
    ];

    for body in &payloads {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/webhook"))
            .header("Content-Type", "application/json")
            .body(*body)
            .send()
            .await
            .expect("HTTP request failed");
    }

    let mut received_subjects = vec![];
    for _ in 0..3 {
        let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .expect("timed out")
            .expect("subscriber closed");
        received_subjects.push(msg.subject.to_string());
    }

    assert_eq!(received_subjects[0], "datadog.alert");
    assert_eq!(received_subjects[1], "datadog.alert.recovered");
    assert_eq!(received_subjects[2], "datadog.alert");
}
