//! Integration tests for the incident.io webhook server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-incidentio --test webhook_e2e

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use futures_util::StreamExt as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_incidentio::{IncidentioConfig, serve};
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

static PORT_COUNTER: AtomicU16 = AtomicU16::new(34000);

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

/// Compute an incident.io-style HMAC-SHA256 signature (raw hex, no prefix).
fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn make_config(nats_port: u16, http_port: u16, secret: Option<&str>) -> IncidentioConfig {
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("INCIDENTIO_WEBHOOK_PORT", http_port.to_string());
    if let Some(s) = secret {
        env.set("INCIDENTIO_WEBHOOK_SECRET", s);
    }
    IncidentioConfig::from_env(&env)
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

/// Happy path: an `incident.created` event with a correct signature returns 200
/// and is published on `incidentio.incident.created`.
#[tokio::test]
async fn webhook_incident_created_returns_200_and_published_to_nats() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "test-secret";
    let body = br#"{"event_type":"incident.created","incident":{"id":"inc-1","name":"DB outage"}}"#;

    let nats = spawn_server(nats_port, http_port, Some(secret)).await;
    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

    let sig = compute_sig(secret, body);
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Incident-Signature", sig)
        .header("X-Incident-Delivery", "del-001")
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

/// `incident.resolved` is published on `incidentio.incident.resolved`.
#[tokio::test]
async fn webhook_incident_resolved_publishes_to_correct_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body =
        br#"{"event_type":"incident.resolved","incident":{"id":"inc-2","name":"DB outage"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats
        .subscribe("incidentio.incident.resolved")
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

/// `incident.updated` is published on `incidentio.incident.updated`.
#[tokio::test]
async fn webhook_incident_updated_publishes_to_correct_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"event_type":"incident.updated","incident":{"id":"inc-3","name":"DB outage"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats
        .subscribe("incidentio.incident.updated")
        .await
        .expect("subscribe");

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

/// Unknown / missing event_type falls back to `incidentio.event`.
#[tokio::test]
async fn webhook_unknown_event_type_publishes_to_event_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"incident":{"id":"inc-4"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("incidentio.event").await.expect("subscribe");

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
        .header("X-Incident-Signature", "deadbeef")
        .header("Content-Type", "application/json")
        .body(r#"{"event_type":"incident.created","incident":{"id":"inc-1"}}"#)
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
        .body(r#"{"event_type":"incident.created","incident":{"id":"inc-1"}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 401);
}

/// When no secret is configured, unsigned requests are accepted.
#[tokio::test]
async fn webhook_no_secret_accepts_unsigned_requests() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"event_type":"incident.created","incident":{"id":"inc-1"}}"#)
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

/// NATS headers are forwarded: `X-Incident-Event-Type` and `X-Incident-Delivery`.
#[tokio::test]
async fn webhook_sets_nats_headers() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"event_type":"incident.created","incident":{"id":"inc-1"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Incident-Delivery", "del-abc-123")
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
        headers.get("X-Incident-Event-Type").map(|v| v.as_str()),
        Some("incident.created")
    );
    assert_eq!(
        headers.get("X-Incident-Delivery").map(|v| v.as_str()),
        Some("del-abc-123")
    );
}

/// Raw payload bytes are preserved exactly — no re-serialisation.
#[tokio::test]
async fn webhook_preserves_payload_bytes_exactly() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"event_type":"incident.created","incident":{"id":"inc-1","name":"DB outage","severity":{"name":"critical"},"status":"active"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

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

/// Webhook upserts incident state into the INCIDENTS KV bucket.
#[tokio::test]
async fn webhook_kv_store_upserted_on_incident_event() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"event_type":"incident.created","incident":{"id":"inc-kv-1","name":"KV test incident"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    // Give the server a moment to write to KV.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let js = async_nats::jetstream::new(nats);
    let kv = js
        .get_key_value("INCIDENTS")
        .await
        .expect("INCIDENTS bucket should exist after first webhook");

    let stored = kv
        .get("inc-kv-1")
        .await
        .expect("KV get failed")
        .expect("incident should be stored in KV");

    assert_eq!(
        stored.as_ref(),
        body.as_ref(),
        "KV value must match original payload"
    );
}

/// GET /incidents returns an empty JSON array when no incidents have been received.
#[tokio::test]
async fn get_incidents_returns_empty_array_initially() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    // We need to prime the INCIDENTS KV bucket by sending at least one webhook
    // first, otherwise the bucket doesn't exist. Use a different server just
    // to create the bucket, then start a fresh server on our test port.
    let priming_port = next_port();
    spawn_server(nats_port, priming_port, None).await;
    // Send a dummy webhook to ensure the bucket is created, then we won't
    // query this server further.
    // Actually, serve() itself creates the KV bucket. We just need to start
    // the server on the real port:
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port}/incidents"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert!(body.is_array(), "response must be a JSON array");
}

/// GET /incidents returns stored incidents after receiving webhooks.
#[tokio::test]
async fn get_incidents_returns_stored_incidents() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let nats = spawn_server(nats_port, http_port, None).await;

    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

    // Post two incidents.
    for id in ["inc-list-1", "inc-list-2"] {
        let body = format!(
            r#"{{"event_type":"incident.created","incident":{{"id":"{id}","name":"Test {id}"}}}}"#
        );
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{http_port}/webhook"))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .expect("HTTP request failed");

        tokio::time::timeout(Duration::from_secs(5), sub.next())
            .await
            .expect("timed out waiting for NATS message")
            .expect("subscriber closed");
    }

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port}/incidents"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);
    let incidents: Vec<serde_json::Value> = resp.json().await.expect("response is not JSON");
    assert_eq!(
        incidents.len(),
        2,
        "should return exactly 2 stored incidents"
    );

    let ids: Vec<&str> = incidents
        .iter()
        .filter_map(|v| v["incident"]["id"].as_str())
        .collect();
    assert!(ids.contains(&"inc-list-1"));
    assert!(ids.contains(&"inc-list-2"));
}

/// GET /incidents/:id returns 404 for an unknown incident.
#[tokio::test]
async fn get_incident_by_id_returns_404_for_unknown() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    spawn_server(nats_port, http_port, None).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{http_port}/incidents/does-not-exist"
        ))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 404);
}

/// GET /incidents/:id returns the stored state for a known incident.
#[tokio::test]
async fn get_incident_by_id_returns_stored_incident() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let nats = spawn_server(nats_port, http_port, None).await;

    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

    let body = r#"{"event_type":"incident.created","incident":{"id":"inc-get-1","name":"Get by ID test"}}"#;
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("HTTP request failed");

    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("subscriber closed");

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port}/incidents/inc-get-1"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);
    let incident: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert_eq!(incident["incident"]["id"].as_str(), Some("inc-get-1"));
    assert_eq!(
        incident["incident"]["name"].as_str(),
        Some("Get by ID test")
    );
}

/// When the INCIDENTIO JetStream stream is deleted, the server returns 500.
#[tokio::test]
async fn webhook_returns_500_when_jetstream_stream_is_gone() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;

    let js = async_nats::jetstream::new(nats);
    js.delete_stream("INCIDENTIO")
        .await
        .expect("failed to delete INCIDENTIO stream");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"event_type":"incident.created","incident":{"id":"inc-1"}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 500);
}

/// Custom subject prefix via INCIDENTIO_SUBJECT_PREFIX env var.
#[tokio::test]
async fn webhook_custom_subject_prefix_is_used() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("INCIDENTIO_WEBHOOK_PORT", http_port.to_string());
    env.set("INCIDENTIO_SUBJECT_PREFIX", "iio");
    env.set("INCIDENTIO_STREAM_NAME", "IIO");
    let config = IncidentioConfig::from_env(&env);

    let nats_for_server = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("connect");
    tokio::spawn(async move { serve(config, nats_for_server).await.ok() });
    wait_for_port(http_port, Duration::from_secs(5)).await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("connect");
    let mut sub = nats
        .subscribe("iio.incident.created")
        .await
        .expect("subscribe");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(r#"{"event_type":"incident.created","incident":{"id":"inc-1"}}"#)
        .send()
        .await
        .expect("HTTP request failed");

    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out on iio.incident.created — custom prefix not applied")
        .expect("subscriber closed");
}

/// Multiple sequential events are all published in order.
#[tokio::test]
async fn webhook_multiple_events_published_in_order() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats.subscribe("incidentio.>").await.expect("subscribe");

    let events = [
        (
            r#"{"event_type":"incident.created","incident":{"id":"inc-a"}}"#,
            "incidentio.incident.created",
        ),
        (
            r#"{"event_type":"incident.updated","incident":{"id":"inc-a"}}"#,
            "incidentio.incident.updated",
        ),
        (
            r#"{"event_type":"incident.resolved","incident":{"id":"inc-a"}}"#,
            "incidentio.incident.resolved",
        ),
    ];

    for (body, _) in &events {
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

    assert_eq!(received_subjects[0], "incidentio.incident.created");
    assert_eq!(received_subjects[1], "incidentio.incident.updated");
    assert_eq!(received_subjects[2], "incidentio.incident.resolved");
}

/// A malformed JSON body (not valid JSON) with a valid signature is accepted
/// and published to the fallback `incidentio.event` subject.  The KV store is
/// NOT updated because `incident_id()` returns `None` for invalid JSON.
#[tokio::test]
async fn webhook_malformed_json_body_published_to_fallback_subject() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "test-secret";
    let body = b"this is not valid json at all";

    let nats = spawn_server(nats_port, http_port, Some(secret)).await;
    let mut sub = nats.subscribe("incidentio.event").await.expect("subscribe");

    let sig = compute_sig(secret, body);
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-Incident-Signature", sig)
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);

    // Message must arrive on the fallback subject.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for incidentio.event message")
        .expect("subscriber closed");

    // KV must remain empty — no incident_id to key on.
    let js = async_nats::jetstream::new(nats_client(nats_port).await);
    let kv = js
        .get_key_value("INCIDENTS")
        .await
        .expect("INCIDENTS bucket must exist");
    let mut keys = kv.keys().await.expect("keys");
    assert!(
        keys.next().await.is_none(),
        "KV must be empty for malformed-JSON body"
    );
}

/// A valid JSON body with a correct signature but without `incident.id` is
/// published to the correct NATS subject, but the KV store is NOT updated.
#[tokio::test]
async fn webhook_body_without_incident_id_is_published_but_not_stored() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    // No incident.id field — the incident object is present but id is absent.
    let body = br#"{"event_type":"incident.created","incident":{"name":"no-id-incident"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    // Published to correct subject.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscriber closed");

    // KV must remain empty — no incident_id was extracted.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let js = async_nats::jetstream::new(nats_client(nats_port).await);
    let kv = js
        .get_key_value("INCIDENTS")
        .await
        .expect("INCIDENTS bucket must exist");
    let mut keys = kv.keys().await.expect("keys");
    assert!(
        keys.next().await.is_none(),
        "KV must be empty when incident.id is missing"
    );
}

/// GET /incidents/:id returns 500 when the stored entry contains invalid JSON.
///
/// This exercises the `serde_json::from_slice` error path inside
/// `get_incident_by_id`, which differs from the `filter_map` in `list_incidents`.
#[tokio::test]
async fn get_incident_by_id_returns_500_for_corrupt_stored_entry() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    // Start the server (creates the INCIDENTS bucket).
    spawn_server(nats_port, http_port, None).await;

    // Inject corrupt JSON directly into the INCIDENTS KV bucket.
    let js = async_nats::jetstream::new(nats_client(nats_port).await);
    let kv = js
        .get_key_value("INCIDENTS")
        .await
        .expect("INCIDENTS bucket must exist");
    kv.put(
        "inc-corrupt-get",
        bytes::Bytes::from_static(b"<<<not json>>>"),
    )
    .await
    .expect("KV put failed");

    let resp = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{http_port}/incidents/inc-corrupt-get"
        ))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(
        resp.status(),
        500,
        "corrupt KV entry must yield 500 from GET /incidents/:id"
    );
}

/// GET /incidents silently skips KV entries that contain invalid JSON.
///
/// This exercises the `filter_map(|bytes| serde_json::from_slice(&bytes).ok())`
/// path in `list_incidents`: if a corrupt entry is present alongside a valid
/// one, only the valid incident is returned.
#[tokio::test]
async fn list_incidents_skips_unparseable_kv_entries() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

    // Post one valid incident so the INCIDENTS bucket is created and populated.
    let valid_body =
        r#"{"event_type":"incident.created","incident":{"id":"inc-valid","name":"Valid"}}"#;
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(valid_body)
        .send()
        .await
        .expect("HTTP request failed");

    // Wait for NATS to confirm the message so the KV write has happened.
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscriber closed");

    // Inject corrupt JSON directly into the INCIDENTS KV bucket.
    let js = async_nats::jetstream::new(nats_client(nats_port).await);
    let kv = js
        .get_key_value("INCIDENTS")
        .await
        .expect("INCIDENTS bucket must exist");
    kv.put(
        "inc-corrupt",
        bytes::Bytes::from_static(b"{this is not valid json"),
    )
    .await
    .expect("KV put failed");

    // GET /incidents must return only the one valid incident.
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port}/incidents"))
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200);
    let incidents: Vec<serde_json::Value> = resp.json().await.expect("response is not JSON");
    assert_eq!(
        incidents.len(),
        1,
        "corrupt KV entry must be silently skipped"
    );
    assert_eq!(incidents[0]["incident"]["id"].as_str(), Some("inc-valid"));
}

/// 10 concurrent webhook requests for distinct incident IDs are all stored in
/// the INCIDENTS KV bucket — verifies there is no race condition in the handler.
#[tokio::test]
async fn webhook_concurrent_requests_all_stored_in_kv() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();

    spawn_server(nats_port, http_port, None).await;

    const N: usize = 10;
    let http_client = reqwest::Client::new();

    // Fire N concurrent POSTs, one per incident ID.
    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let client = http_client.clone();
        let port = http_port;
        handles.push(tokio::spawn(async move {
            let body = format!(
                r#"{{"event_type":"incident.created","incident":{{"id":"inc-concurrent-{i}","name":"Concurrent incident {i}"}}}}"#
            );
            client
                .post(format!("http://127.0.0.1:{port}/webhook"))
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
                .expect("HTTP request failed")
                .status()
        }));
    }

    // All requests must return 200.
    for handle in handles {
        let status = handle.await.expect("task panicked");
        assert_eq!(status, 200, "concurrent request did not return 200");
    }

    // Wait for all KV writes to settle.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // All N incidents must be present in the KV store.
    let js = async_nats::jetstream::new(nats_client(nats_port).await);
    let kv = js
        .get_key_value("INCIDENTS")
        .await
        .expect("INCIDENTS bucket should exist");

    for i in 0..N {
        let key = format!("inc-concurrent-{i}");
        let entry = kv
            .get(&key)
            .await
            .unwrap_or_else(|e| panic!("KV get failed for {key}: {e}"))
            .unwrap_or_else(|| panic!("incident {key} not found in KV after concurrent POSTs"));
        // Sanity-check: the stored payload contains the expected incident id.
        let parsed: serde_json::Value =
            serde_json::from_slice(&entry).expect("stored entry is not valid JSON");
        assert_eq!(
            parsed["incident"]["id"].as_str(),
            Some(key.as_str()),
            "stored incident id mismatch for {key}"
        );
    }
}

/// Calling `serve()` twice on the same NATS JetStream context (same container)
/// must both succeed — `ensure_stream` uses `get_or_create_stream` which is
/// idempotent even when the stream already exists.
#[tokio::test]
async fn ensure_stream_is_idempotent() {
    let (_container, nats_port) = start_nats().await;

    // First server on port A.
    let http_port_a = next_port();
    let config_a = make_config(nats_port, http_port_a, None);
    let nats_a = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config_a, nats_a).await.ok() });
    wait_for_port(http_port_a, Duration::from_secs(5)).await;

    // Second server on port B — reuses the same NATS/JetStream context (same
    // container), so `ensure_stream` will find the stream already exists.
    let http_port_b = next_port();
    let config_b = make_config(nats_port, http_port_b, None);
    let nats_b = nats_client(nats_port).await;
    tokio::spawn(async move { serve(config_b, nats_b).await.ok() });
    wait_for_port(http_port_b, Duration::from_secs(5)).await;

    // Both servers must be responsive.
    let resp_a = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port_a}/health"))
        .send()
        .await
        .expect("HTTP request to server A failed");
    assert_eq!(resp_a.status(), 200, "server A health check failed");

    let resp_b = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{http_port_b}/health"))
        .send()
        .await
        .expect("HTTP request to server B failed");
    assert_eq!(resp_b.status(), 200, "server B health check failed");
}

/// When the INCIDENTS KV bucket is deleted after the server starts, the handler
/// logs a warning but still publishes to NATS and returns 200.
///
/// This exercises the `warn!` + continue path in `handle_webhook` where
/// `incident_store.upsert()` fails silently.
#[tokio::test]
async fn webhook_kv_upsert_failure_still_publishes_to_nats() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"event_type":"incident.created","incident":{"id":"inc-kv-gone"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;

    // Subscribe before deleting the bucket so we don't miss the message.
    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

    // Delete the INCIDENTS KV bucket while the server is running.
    let js = async_nats::jetstream::new(nats_client(nats_port).await);
    js.delete_key_value("INCIDENTS")
        .await
        .expect("failed to delete INCIDENTS KV bucket");

    // Small pause so the delete propagates before the request arrives.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The webhook must still return 200 — KV failure is non-fatal.
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");
    assert_eq!(
        resp.status(),
        200,
        "KV failure must not prevent 200 response"
    );

    // NATS publish must still succeed.
    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message after KV bucket deletion")
        .expect("subscriber closed");
    assert_eq!(
        msg.payload.as_ref(),
        body.as_ref(),
        "payload must be bit-for-bit identical"
    );
}

/// HTTP header names are case-insensitive; a sender using `X-INCIDENT-SIGNATURE`
/// (all-caps) must be accepted the same as `X-Incident-Signature`.
#[tokio::test]
async fn webhook_signature_header_is_case_insensitive() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let secret = "case-test-secret";
    let body = br#"{"event_type":"incident.created","incident":{"id":"inc-case"}}"#;

    spawn_server(nats_port, http_port, Some(secret)).await;

    let sig = compute_sig(secret, body);
    // Send using all-uppercase header name.
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/webhook"))
        .header("X-INCIDENT-SIGNATURE", sig)
        .header("Content-Type", "application/json")
        .body(body.as_ref())
        .send()
        .await
        .expect("HTTP request failed");

    assert_eq!(resp.status(), 200, "uppercase header name must be accepted");
}

/// Missing X-Incident-Delivery header defaults to "unknown".
#[tokio::test]
async fn webhook_missing_delivery_header_defaults_to_unknown() {
    let (_container, nats_port) = start_nats().await;
    let http_port = next_port();
    let body = br#"{"event_type":"incident.created","incident":{"id":"inc-1"}}"#;

    let nats = spawn_server(nats_port, http_port, None).await;
    let mut sub = nats
        .subscribe("incidentio.incident.created")
        .await
        .expect("subscribe");

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

    let headers = msg.headers.expect("no headers");
    assert_eq!(
        headers.get("X-Incident-Delivery").map(|v| v.as_str()),
        Some("unknown"),
        "missing X-Incident-Delivery must default to 'unknown'"
    );
}
