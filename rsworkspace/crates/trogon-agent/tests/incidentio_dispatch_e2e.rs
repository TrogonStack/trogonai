//! Cross-crate e2e: incident.io HTTP webhook → JetStream → agent → proxy → worker → mock AI.
//!
//! ```text
//! POST /webhook (with HMAC-SHA256 incident.io signature)
//!   → trogon-incidentio server → JetStream INCIDENTIO stream
//!   → trogon-agent runner (incident_declared handler)
//!   → AgentLoop → POST proxy/anthropic/v1/messages (Bearer tok_anthropic_prod_test01)
//!   → trogon-secret-proxy HTTP server
//!   → PROXY_REQUESTS JetStream stream
//!   → worker → resolves tok_anthropic_prod_test01 → sk-ant-realkey
//!   → mock Anthropic (receives Bearer sk-ant-realkey)
//!   → end_turn → runner acks message
//! ```
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test incidentio_dispatch_e2e

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use hmac::{Hmac, Mac};
use httpmock::MockServer;
use sha2::Sha256;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use trogon_agent::{AgentConfig, run};
use trogon_incidentio::{IncidentioConfig, serve as incidentio_serve};
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_secret_proxy::{proxy::{ProxyState, router}, stream, subjects, worker};
use trogon_std::env::InMemoryEnv;
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

type HmacSha256 = Hmac<Sha256>;

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

static PORT_COUNTER: AtomicU16 = AtomicU16::new(36000);

fn next_port() -> u16 {
    PORT_COUNTER.fetch_add(1, Ordering::SeqCst)
}

async fn wait_for_port(port: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("Port {port} not ready within 5 s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// incident.io signature: raw HMAC-SHA256 hex (no prefix).
fn compute_incidentio_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

async fn wait_for_hit(mock: &httpmock::Mock<'_>, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if mock.hits_async().await >= 1 {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn base_agent_config(nats_port: u16, proxy_url: String) -> AgentConfig {
    AgentConfig {
        nats: NatsConfig::new(vec![format!("nats://127.0.0.1:{nats_port}")], NatsAuth::None),
        proxy_url,
        anthropic_token: "tok_anthropic_prod_test01".to_string(),
        github_token: String::new(),
        linear_token: String::new(),
        slack_token: String::new(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 1,
        github_stream_name: None,
        linear_stream_name: None,
        cron_stream_name: None,
        datadog_stream_name: None,
        incidentio_stream_name: None,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        split_evaluator_url: None,
        split_auth_token: None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Full pipeline: incident.io `incident.created` webhook → trogon-incidentio →
/// JetStream INCIDENTIO stream → trogon-agent runner (incident_declared handler)
/// → proxy → worker → mock Anthropic (receives REAL API key, not opaque token).
#[tokio::test]
async fn incidentio_webhook_triggers_full_pipeline_with_real_key() {
    // ── 1. Start NATS ──────────────────────────────────────────────────────
    let (_nats_container, nats_port) = start_nats().await;

    // ── 2. Mock Anthropic — must receive REAL key ──────────────────────────
    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Incident acknowledged."}]}"#,
                );
        })
        .await;

    // ── 3. Seed vault ──────────────────────────────────────────────────────
    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    // ── 4. NATS connections ────────────────────────────────────────────────
    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("Failed to connect to NATS");
    let js = Arc::new(jetstream::new(nats.clone()));

    // ── 5. Ensure PROXY_REQUESTS stream ────────────────────────────────────
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .expect("Failed to ensure PROXY_REQUESTS stream");

    // ── 6. Start proxy + worker ────────────────────────────────────────────
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: Arc::clone(&js),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state))
            .await
            .expect("Proxy server error");
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let worker_js = Arc::clone(&js);
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            http_client,
            "incidentio-pipeline-worker",
            &worker_stream,
        )
        .await
        .expect("Worker error");
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── 7. Start trogon-incidentio webhook server ──────────────────────────
    let incidentio_port = next_port();
    let webhook_secret = "test-incidentio-secret";

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("INCIDENTIO_WEBHOOK_PORT", incidentio_port.to_string());
    env.set("INCIDENTIO_WEBHOOK_SECRET", webhook_secret);
    let incidentio_config = IncidentioConfig::from_env(&env);

    let nats_for_incidentio = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        incidentio_serve(incidentio_config, nats_for_incidentio)
            .await
            .expect("incident.io server error");
    });

    wait_for_port(incidentio_port).await;

    // ── 8. Start agent runner ──────────────────────────────────────────────
    let agent_cfg = base_agent_config(nats_port, format!("http://127.0.0.1:{proxy_port}"));
    tokio::spawn(async move { run(agent_cfg).await.ok(); });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 9. POST an incident.created webhook ────────────────────────────────
    let body = serde_json::to_vec(&serde_json::json!({
        "event_type": "incident.created",
        "incident": {
            "id": "inc-e2e-001",
            "name": "Database connection pool exhausted",
            "status": "active",
            "severity": {
                "name": "critical"
            }
        }
    }))
    .unwrap();

    let sig = compute_incidentio_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{incidentio_port}/webhook"))
        .header("x-incident-signature", sig)
        .header("x-incident-delivery", "del-e2e-001")
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("POST to incident.io webhook server failed");

    assert_eq!(resp.status(), 200);

    // ── 10. Assert: mock Anthropic received the REAL key ──────────────────
    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Mock Anthropic was not called within 20 s — the full pipeline \
         (webhook → incidentio → JetStream → agent → proxy → worker → mock) may not be wired correctly"
    );

    anthropic_mock.assert_async().await;
}

/// Same pipeline but with `incident.resolved` — verifies the subject
/// `incidentio.incident.resolved` also reaches the agent handler.
#[tokio::test]
async fn incidentio_resolved_triggers_full_pipeline() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("resolved");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Incident resolved, post-mortem scheduled."}]}"#,
                );
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: Arc::clone(&js),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move { axum::serve(listener, router(proxy_state)).await.ok(); });

    let http_client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap();
    let wjs = Arc::clone(&js);
    let wnats = nats.clone();
    let wvault = Arc::clone(&vault);
    let wstream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(wjs, wnats, wvault, http_client, "iio-resolved-worker", &wstream).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let incidentio_port = next_port();
    let webhook_secret = "test-iio-resolved-secret";
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("INCIDENTIO_WEBHOOK_PORT", incidentio_port.to_string());
    env.set("INCIDENTIO_WEBHOOK_SECRET", webhook_secret);
    let incidentio_config = IncidentioConfig::from_env(&env);

    let nats_for_iio = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    tokio::spawn(async move { incidentio_serve(incidentio_config, nats_for_iio).await.ok(); });
    wait_for_port(incidentio_port).await;

    let agent_cfg = base_agent_config(nats_port, format!("http://127.0.0.1:{proxy_port}"));
    tokio::spawn(async move { run(agent_cfg).await.ok(); });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "event_type": "incident.resolved",
        "incident": {
            "id": "inc-e2e-resolved-001",
            "name": "Database connection pool exhausted",
            "status": "resolved",
            "severity": { "name": "critical" }
        }
    }))
    .unwrap();
    let sig = compute_incidentio_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{incidentio_port}/webhook"))
        .header("x-incident-signature", sig)
        .header("x-incident-delivery", "del-resolved-001")
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(hit, "Mock Anthropic not called for incident.resolved within 20 s");
    anthropic_mock.assert_async().await;
}

/// Same pipeline but with `incident.updated` — verifies the subject
/// `incidentio.incident.updated` reaches the agent (handled by the `_` branch).
#[tokio::test]
async fn incidentio_updated_triggers_full_pipeline() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("incident.updated");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Status update posted."}]}"#,
                );
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: Arc::clone(&js),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move { axum::serve(listener, router(proxy_state)).await.ok(); });

    let http_client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap();
    let wjs = Arc::clone(&js);
    let wnats = nats.clone();
    let wvault = Arc::clone(&vault);
    let wstream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(wjs, wnats, wvault, http_client, "iio-updated-worker", &wstream).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let incidentio_port = next_port();
    let webhook_secret = "test-iio-updated-secret";
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("INCIDENTIO_WEBHOOK_PORT", incidentio_port.to_string());
    env.set("INCIDENTIO_WEBHOOK_SECRET", webhook_secret);
    let incidentio_config = IncidentioConfig::from_env(&env);

    let nats_for_iio = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    tokio::spawn(async move { incidentio_serve(incidentio_config, nats_for_iio).await.ok(); });
    wait_for_port(incidentio_port).await;

    let agent_cfg = base_agent_config(nats_port, format!("http://127.0.0.1:{proxy_port}"));
    tokio::spawn(async move { run(agent_cfg).await.ok(); });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "event_type": "incident.updated",
        "incident": {
            "id": "inc-e2e-updated-001",
            "name": "Memory leak in service",
            "status": "investigating",
            "severity": { "name": "high" }
        }
    }))
    .unwrap();
    let sig = compute_incidentio_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{incidentio_port}/webhook"))
        .header("x-incident-signature", sig)
        .header("x-incident-delivery", "del-updated-001")
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(hit, "Mock Anthropic not called for incident.updated within 20 s");
    anthropic_mock.assert_async().await;
}

/// Automation registered for `incidentio.incident.created` takes precedence
/// over the fallback handler.
#[tokio::test]
async fn incidentio_automation_dispatch_takes_precedence_over_fallback() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("incidentio.incident.created");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "automation ran for incident"}]
                }));
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    // Register a matching automation.
    let store = trogon_automations::AutomationStore::open(&js).await.unwrap();
    let auto = trogon_automations::Automation {
        id: "iio-auto-1".to_string(),
        tenant_id: "default".to_string(),
        name: "incident.io incident auto".to_string(),
        trigger: "incidentio.incident.created".to_string(),
        prompt: "Handle this incident.io event via automation.".to_string(),
        model: None,
        tools: vec![],
        memory_path: None,
        mcp_servers: vec![],
        enabled: true,
        visibility: trogon_automations::Visibility::Private,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    };
    store.put(&auto).await.unwrap();

    // Pre-create the INCIDENTIO stream so trogon-incidentio can publish.
    js.get_or_create_stream(jetstream::stream::Config {
        name: "INCIDENTIO".to_string(),
        subjects: vec!["incidentio.>".to_string()],
        ..Default::default()
    })
    .await
    .unwrap();

    let incidentio_port = next_port();
    let webhook_secret = "test-iio-auto-secret";
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("INCIDENTIO_WEBHOOK_PORT", incidentio_port.to_string());
    env.set("INCIDENTIO_WEBHOOK_SECRET", webhook_secret);
    let incidentio_config = IncidentioConfig::from_env(&env);

    let nats_for_iio = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    tokio::spawn(async move { incidentio_serve(incidentio_config, nats_for_iio).await.ok(); });
    wait_for_port(incidentio_port).await;

    let agent_cfg = base_agent_config(nats_port, mock_server.base_url());
    tokio::spawn(async move { run(agent_cfg).await.ok(); });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let body = serde_json::to_vec(&serde_json::json!({
        "event_type": "incident.created",
        "incident": {
            "id": "inc-auto-001",
            "name": "Automation dispatch test",
            "status": "active",
            "severity": { "name": "low" }
        }
    }))
    .unwrap();
    let sig = compute_incidentio_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{incidentio_port}/webhook"))
        .header("x-incident-signature", sig)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(hit, "Automation was not dispatched for incidentio.incident.created within 20 s");
    anthropic_mock.assert_async().await;
}
