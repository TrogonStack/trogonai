//! Cross-crate e2e: Datadog HTTP webhook → JetStream → agent → proxy → worker → mock AI.
//!
//! ```text
//! POST /webhook (with HMAC-SHA256 Datadog signature)
//!   → trogon-datadog server → JetStream DATADOG stream
//!   → trogon-agent runner (alert_triggered handler)
//!   → AgentLoop → POST proxy/anthropic/v1/messages (Bearer tok_anthropic_prod_test01)
//!   → trogon-secret-proxy HTTP server
//!   → PROXY_REQUESTS JetStream stream
//!   → worker → resolves tok_anthropic_prod_test01 → sk-ant-realkey
//!   → mock Anthropic (receives Bearer sk-ant-realkey)
//!   → end_turn → runner acks message
//! ```
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test datadog_dispatch_e2e

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use async_nats::jetstream;
use hmac::{Hmac, Mac};
use httpmock::MockServer;
use sha2::Sha256;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent::{AgentConfig, run};
use trogon_datadog::{DatadogConfig, serve as datadog_serve};
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, worker,
};
use trogon_std::env::InMemoryEnv;
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

type HmacSha256 = Hmac<Sha256>;

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

/// Unique port counter (starts high to avoid conflicts with other test suites).
static PORT_COUNTER: AtomicU16 = AtomicU16::new(33000);

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

/// Datadog signature: raw HMAC-SHA256 hex (no prefix).
fn compute_datadog_sig(secret: &str, body: &[u8]) -> String {
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

// ── Tests ────────────────────────────────────────────────────────────────────

/// Full pipeline: Datadog HTTP webhook → trogon-datadog → JetStream DATADOG
/// stream → trogon-agent runner (alert_triggered handler) → proxy → worker →
/// mock Anthropic (receives REAL API key, not opaque token).
#[tokio::test]
async fn datadog_webhook_http_triggers_full_pipeline_with_real_key() {
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
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Alert acknowledged."}]}"#,
                );
        })
        .await;

    // ── 3. Seed vault ──────────────────────────────────────────────────────
    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_prod_test01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

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
            "datadog-pipeline-worker",
            &worker_stream,
        )
        .await
        .expect("Worker error");
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── 7. Start trogon-datadog webhook server ─────────────────────────────
    let datadog_port = next_port();
    let webhook_secret = "test-datadog-secret";

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("DATADOG_WEBHOOK_PORT", datadog_port.to_string());
    env.set("DATADOG_WEBHOOK_SECRET", webhook_secret);
    let datadog_config = DatadogConfig::from_env(&env);

    let nats_for_datadog = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        datadog_serve(datadog_config, nats_for_datadog)
            .await
            .expect("Datadog server error");
    });

    wait_for_port(datadog_port).await;

    // The agent runner creates GITHUB, LINEAR, CRON_TICKS, and DATADOG streams
    // idempotently via ensure_stream, so no pre-creation is needed here.

    // ── 8. Start agent runner ──────────────────────────────────────────────
    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: format!("http://127.0.0.1:{proxy_port}"),
        anthropic_token: "tok_anthropic_prod_test01".to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: String::new(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 1,
        github_stream_name: None,
        linear_stream_name: None,
        cron_stream_name: None,
        datadog_stream_name: None,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    // Give the agent runner a moment to bind its consumers.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 9. POST a Datadog "Triggered" alert webhook ────────────────────────
    let body = serde_json::to_vec(&serde_json::json!({
        "alert_transition": "Triggered",
        "alert_id": "1234567890",
        "alert_title": "CPU usage is too high on web-01",
        "alert_metric": "system.cpu.user",
        "host": "web-01",
        "priority": "normal",
        "date_happened": 1700000000u64
    }))
    .unwrap();

    let sig = compute_datadog_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{datadog_port}/webhook"))
        .header("x-datadog-signature", sig)
        .header("dd-request-id", "dd-req-e2e-001")
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("POST to Datadog webhook server failed");

    assert_eq!(
        resp.status(),
        200,
        "Datadog webhook server must return 200 for a valid signed request"
    );

    // ── 10. Assert: mock Anthropic received the REAL key ──────────────────
    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Mock Anthropic was not called within 20 s — the full pipeline \
         (webhook → datadog → JetStream → agent → proxy → worker → mock) may not be wired correctly"
    );

    anthropic_mock.assert_async().await;
}

/// Same pipeline but with a "Recovered" transition — verifies the subject
/// routing `datadog.alert.recovered` also reaches the agent handler.
#[tokio::test]
async fn datadog_recovered_alert_triggers_full_pipeline() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("Recovered");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Recovery noted."}]}"#,
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

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let wjs = Arc::clone(&js);
    let wnats = nats.clone();
    let wvault = Arc::clone(&vault);
    let wstream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            wjs,
            wnats,
            wvault,
            http_client,
            "dd-recovered-worker",
            &wstream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let datadog_port = next_port();
    let webhook_secret = "test-dd-recovered-secret";
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("DATADOG_WEBHOOK_PORT", datadog_port.to_string());
    env.set("DATADOG_WEBHOOK_SECRET", webhook_secret);
    let datadog_config = DatadogConfig::from_env(&env);

    let nats_for_dd = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        datadog_serve(datadog_config, nats_for_dd).await.ok();
    });

    wait_for_port(datadog_port).await;

    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: format!("http://127.0.0.1:{proxy_port}"),
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
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // POST a "Recovered" alert
    let body = serde_json::to_vec(&serde_json::json!({
        "alert_transition": "Recovered",
        "alert_id": "9876543210",
        "alert_title": "CPU usage back to normal on web-01",
        "alert_metric": "system.cpu.user",
        "host": "web-01"
    }))
    .unwrap();
    let sig = compute_datadog_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{datadog_port}/webhook"))
        .header("x-datadog-signature", sig)
        .header("dd-request-id", "dd-req-e2e-recovered")
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Mock Anthropic not called for Recovered alert within 20 s"
    );
    anthropic_mock.assert_async().await;
}

/// Verify that the agent runner's automation dispatch path works for Datadog:
/// when a matching automation exists, it runs it (not the fallback handler).
#[tokio::test]
async fn datadog_automation_dispatch_takes_precedence_over_fallback() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    // The automation prompt body_contains check: the automation's prompt text
    // must appear in the Anthropic request.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("datadog.alert");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "automation ran"}]
                }));
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    // ── Register a matching automation in the store ──
    let store = trogon_automations::AutomationStore::open(&js)
        .await
        .unwrap();
    let auto = trogon_automations::Automation {
        id: "dd-auto-1".to_string(),
        tenant_id: "default".to_string(),
        name: "DD alert auto".to_string(),
        trigger: "datadog.alert".to_string(),
        prompt: "Handle this Datadog alert via automation.".to_string(),
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

    // Ensure DATADOG stream exists (the runner will also ensure it, but
    // we need it before the datadog server can publish to it).
    js.get_or_create_stream(jetstream::stream::Config {
        name: "DATADOG".to_string(),
        subjects: vec!["datadog.>".to_string()],
        ..Default::default()
    })
    .await
    .unwrap();

    let datadog_port = next_port();
    let webhook_secret = "test-dd-auto-secret";
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("DATADOG_WEBHOOK_PORT", datadog_port.to_string());
    env.set("DATADOG_WEBHOOK_SECRET", webhook_secret);
    let datadog_config = DatadogConfig::from_env(&env);

    let nats_for_dd = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        datadog_serve(datadog_config, nats_for_dd).await.ok();
    });
    wait_for_port(datadog_port).await;

    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: mock_server.base_url(),
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
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // POST a "Triggered" alert — should hit the automation, not the fallback handler.
    let body = serde_json::to_vec(&serde_json::json!({
        "alert_transition": "Triggered",
        "alert_id": "auto-test-alert",
        "alert_title": "Test alert for automation dispatch",
        "host": "web-01"
    }))
    .unwrap();
    let sig = compute_datadog_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{datadog_port}/webhook"))
        .header("x-datadog-signature", sig)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Automation was not dispatched for Datadog alert within 20 s"
    );
    anthropic_mock.assert_async().await;
}

/// Full pipeline with a non-alert Datadog event (`datadog.event` subject).
/// Payload has no `alert_transition` field — trogon-datadog routes it to
/// `datadog.event`. The agent's fallback handler must pick it up and call
/// the Anthropic API.
#[tokio::test]
async fn datadog_event_subject_triggers_full_pipeline() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Event logged."}]}"#,
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

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let wjs = Arc::clone(&js);
    let wnats = nats.clone();
    let wvault = Arc::clone(&vault);
    let wstream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(wjs, wnats, wvault, http_client, "dd-event-worker", &wstream)
            .await
            .ok();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let datadog_port = next_port();
    let webhook_secret = "test-dd-event-secret";
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("DATADOG_WEBHOOK_PORT", datadog_port.to_string());
    env.set("DATADOG_WEBHOOK_SECRET", webhook_secret);
    let datadog_config = DatadogConfig::from_env(&env);

    let nats_for_dd = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        datadog_serve(datadog_config, nats_for_dd).await.ok();
    });
    wait_for_port(datadog_port).await;

    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: format!("http://127.0.0.1:{proxy_port}"),
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
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // POST a payload with NO alert_transition → trogon-datadog routes to datadog.event.
    let body = serde_json::to_vec(&serde_json::json!({
        "monitor_name": "Custom monitor fired",
        "monitor_id": 99999,
        "tags": ["env:prod", "service:web"]
    }))
    .unwrap();
    let sig = compute_datadog_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{datadog_port}/webhook"))
        .header("x-datadog-signature", sig)
        .header("dd-request-id", "dd-req-event-001")
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Mock Anthropic not called for datadog.event within 20 s — \
         the datadog.event subject path may not be handled by the agent runner"
    );
    anthropic_mock.assert_async().await;
}

/// When the Anthropic API returns an error for a Datadog message, the runner
/// must still ack the message and continue processing subsequent messages.
/// Verified by posting two messages and asserting both reach Anthropic.
#[tokio::test]
async fn datadog_handler_error_still_acks_and_continues() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    // Both calls return 500. If the runner deadlocks or panics on the first
    // failure, the second message will never be processed within the timeout.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(500)
                .header("content-type", "application/json")
                .body(r#"{"error":"internal server error"}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    js.get_or_create_stream(jetstream::stream::Config {
        name: "DATADOG".to_string(),
        subjects: vec!["datadog.>".to_string()],
        ..Default::default()
    })
    .await
    .unwrap();

    let datadog_port = next_port();
    let webhook_secret = "test-dd-err-secret";
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("DATADOG_WEBHOOK_PORT", datadog_port.to_string());
    env.set("DATADOG_WEBHOOK_SECRET", webhook_secret);
    let datadog_config = DatadogConfig::from_env(&env);

    let nats_for_dd = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        datadog_serve(datadog_config, nats_for_dd).await.ok();
    });
    wait_for_port(datadog_port).await;

    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: mock_server.base_url(),
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
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // POST two messages back-to-back.
    let client = reqwest::Client::new();
    for i in 0..2u32 {
        let body = serde_json::to_vec(&serde_json::json!({
            "alert_transition": "Triggered",
            "alert_id": format!("err-test-{i}"),
            "alert_title": "Error resilience test alert",
            "host": "web-01"
        }))
        .unwrap();
        let sig = compute_datadog_sig(webhook_secret, &body);
        client
            .post(format!("http://127.0.0.1:{datadog_port}/webhook"))
            .header("x-datadog-signature", sig)
            .header("Content-Type", "application/json")
            .body(body)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .unwrap();
    }

    // Both messages must reach Anthropic — the runner must not deadlock
    // or stop processing after the first error.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if anthropic_mock.hits_async().await >= 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Runner did not dispatch both Datadog messages within 20 s \
             after Anthropic errors — possible ack/continuation failure"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Automation registered for `datadog.event` takes precedence over the
/// fallback handler when a non-alert Datadog event is received.
#[tokio::test]
async fn datadog_event_automation_dispatch() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    // The automation prompt contains "datadog.event" because run_automation
    // prepends the NATS subject to the prompt.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("datadog.event");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "event automation ran"}]
                }));
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    // Register automation matching datadog.event.
    let store = trogon_automations::AutomationStore::open(&js)
        .await
        .unwrap();
    let auto = trogon_automations::Automation {
        id: "dd-event-auto-1".to_string(),
        tenant_id: "default".to_string(),
        name: "DD event auto".to_string(),
        trigger: "datadog.event".to_string(),
        prompt: "Handle this Datadog event via automation.".to_string(),
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

    js.get_or_create_stream(jetstream::stream::Config {
        name: "DATADOG".to_string(),
        subjects: vec!["datadog.>".to_string()],
        ..Default::default()
    })
    .await
    .unwrap();

    let datadog_port = next_port();
    let webhook_secret = "test-dd-event-auto-secret";
    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("DATADOG_WEBHOOK_PORT", datadog_port.to_string());
    env.set("DATADOG_WEBHOOK_SECRET", webhook_secret);
    let datadog_config = DatadogConfig::from_env(&env);

    let nats_for_dd = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        datadog_serve(datadog_config, nats_for_dd).await.ok();
    });
    wait_for_port(datadog_port).await;

    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: mock_server.base_url(),
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
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // POST a payload with no alert_transition → trogon-datadog routes to datadog.event.
    let body = serde_json::to_vec(&serde_json::json!({
        "monitor_name": "Custom event monitor",
        "monitor_id": 88888
    }))
    .unwrap();
    let sig = compute_datadog_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{datadog_port}/webhook"))
        .header("x-datadog-signature", sig)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Automation was not dispatched for datadog.event within 20 s"
    );
    anthropic_mock.assert_async().await;
}
