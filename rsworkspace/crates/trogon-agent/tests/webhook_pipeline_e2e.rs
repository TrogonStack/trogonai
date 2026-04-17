//! Cross-crate e2e: HTTP webhook → JetStream → agent → proxy → worker → mock AI.
//!
//! Closes the remaining gap: `pipeline_e2e.rs` injects events directly into
//! JetStream, bypassing `trogon-github`.  This file starts the real GitHub
//! webhook HTTP server so the full path from an external HTTP call all the
//! way to the Anthropic API call is exercised end-to-end.
//!
//! ```text
//! POST /webhook (with HMAC signature)
//!   → trogon-github server → JetStream GITHUB stream
//!   → trogon-agent runner (PR review handler)
//!   → AgentLoop → POST proxy/anthropic/v1/messages (Bearer tok_anthropic_prod_test01)
//!   → trogon-secret-proxy HTTP server
//!   → PROXY_REQUESTS JetStream stream
//!   → worker → resolves tok_anthropic_prod_test01 → sk-ant-realkey
//!   → mock Anthropic (receives Bearer sk-ant-realkey)
//!   → end_turn → runner acks message
//! ```
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test webhook_pipeline_e2e

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
use trogon_automations::{Automation, AutomationStore};
use trogon_github::{GithubConfig, serve as github_serve};
use trogon_linear::{LinearConfig, serve as linear_serve};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, worker,
};
use trogon_std::env::InMemoryEnv;
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

type HmacSha256 = Hmac<Sha256>;

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

/// Unique port counter (starts high to avoid conflicts with other test suites).
static PORT_COUNTER: AtomicU16 = AtomicU16::new(31000);

fn next_port() -> u16 {
    PORT_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Busy-waits until the TCP port is accepting connections.
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

fn compute_sig(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Linear signature is raw hex HMAC-SHA256 (no "sha256=" prefix).
fn compute_linear_sig(secret: &str, body: &[u8]) -> String {
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

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Full 5-piece pipeline: HTTP webhook → trogon-github → JetStream → agent →
/// proxy → worker → mock Anthropic with the REAL api key.
///
/// This is the only test that exercises the `trogon-github` HTTP layer as part
/// of the agentic pipeline (other tests inject directly into JetStream).
#[tokio::test]
async fn webhook_http_triggers_full_pipeline_with_real_key() {
    // ── 1. Start NATS ──────────────────────────────────────────────────────
    let (_nats_container, nats_port) = start_nats().await;

    // ── 2. Mock Anthropic — must receive REAL key, not opaque token ────────
    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"LGTM."}]}"#);
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
    let js = NatsJetStreamClient::new(jetstream::new(nats.clone()));

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
        jetstream: js.clone(),
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
    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            http_client,
            "webhook-pipeline-worker",
            &worker_stream,
        )
        .await
        .expect("Worker error");
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── 7. Start trogon-github webhook server ──────────────────────────────
    let github_port = next_port();
    let webhook_secret = "test-webhook-secret";

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("GITHUB_WEBHOOK_PORT", github_port.to_string());
    env.set("GITHUB_WEBHOOK_SECRET", webhook_secret);
    // Use the default stream name "GITHUB" and subject prefix "github"
    let github_config = GithubConfig::from_env(&env);

    let nats_for_github = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        github_serve(github_config, nats_for_github)
            .await
            .expect("GitHub server error");
    });

    // Wait for the GitHub server to start accepting connections.
    wait_for_port(github_port).await;

    // The agent runner requires GITHUB, LINEAR, and CRON_TICKS streams to exist.
    // The GitHub server created GITHUB above; create the others manually.
    js.context()
        .get_or_create_stream(jetstream::stream::Config {
            name: "LINEAR".to_string(),
            subjects: vec!["linear.Issue.>".to_string()],
            ..Default::default()
        })
        .await
        .expect("Failed to create LINEAR stream");
    js.context()
        .get_or_create_stream(jetstream::stream::Config {
            name: "CRON_TICKS".to_string(),
            subjects: vec!["cron.>".to_string()],
            ..Default::default()
        })
        .await
        .expect("Failed to create CRON_TICKS stream");

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

    // ── 9. POST a GitHub pull_request webhook ──────────────────────────────
    let body = serde_json::to_vec(&serde_json::json!({
        "action": "opened",
        "number": 7,
        "repository": {
            "name": "trogon",
            "owner": { "login": "acme" }
        }
    }))
    .unwrap();

    let sig = compute_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{github_port}/webhook"))
        .header("X-GitHub-Event", "pull_request")
        .header("X-GitHub-Delivery", "delivery-001")
        .header("X-Hub-Signature-256", sig)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("POST to GitHub webhook server failed");

    assert_eq!(
        resp.status(),
        200,
        "GitHub webhook server must return 200 for a valid signed request"
    );

    // ── 10. Assert: mock Anthropic received the REAL key ──────────────────
    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Mock Anthropic was not called within 20 s — the full 5-piece pipeline \
         (webhook → github → JetStream → agent → proxy → worker → mock) may not be wired correctly"
    );

    anthropic_mock.assert_async().await;
}

/// Full 5-piece pipeline via the Linear webhook HTTP server:
/// POST /webhook (with HMAC-SHA256 linear-signature)
///   → trogon-linear server → JetStream LINEAR stream
///   → trogon-agent runner (issue triage handler)
///   → AgentLoop → proxy → worker → mock Anthropic (Bearer sk-ant-realkey)
#[tokio::test]
async fn linear_webhook_http_triggers_full_pipeline_with_real_key() {
    // ── 1. Start NATS ──────────────────────────────────────────────────────
    let (_nats_container, nats_port) = start_nats().await;

    // ── 2. Mock Anthropic — must receive REAL key, not opaque token ────────
    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Triaged."}]}"#,
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
    let js = NatsJetStreamClient::new(jetstream::new(nats.clone()));

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
        jetstream: js.clone(),
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
    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            http_client,
            "linear-webhook-pipeline-worker",
            &worker_stream,
        )
        .await
        .expect("Worker error");
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── 7. Start trogon-linear webhook server ──────────────────────────────
    let linear_port = next_port();
    let webhook_secret = "test-linear-webhook-secret";

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", linear_port.to_string());
    env.set("LINEAR_WEBHOOK_SECRET", webhook_secret);
    // Disable timestamp tolerance so the test doesn't need a live clock.
    env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "0");
    let linear_config = LinearConfig::from_env(&env);

    let nats_for_linear = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        linear_serve(linear_config, nats_for_linear)
            .await
            .expect("Linear server error");
    });

    // Wait for the Linear server to start accepting connections.
    wait_for_port(linear_port).await;

    // The agent runner requires GITHUB, LINEAR, and CRON_TICKS streams to exist.
    // The Linear server created LINEAR above; create the others manually.
    js.context()
        .get_or_create_stream(jetstream::stream::Config {
            name: "GITHUB".to_string(),
            subjects: vec!["github.pull_request".to_string()],
            ..Default::default()
        })
        .await
        .expect("Failed to create GITHUB stream");
    js.context()
        .get_or_create_stream(jetstream::stream::Config {
            name: "CRON_TICKS".to_string(),
            subjects: vec!["cron.>".to_string()],
            ..Default::default()
        })
        .await
        .expect("Failed to create CRON_TICKS stream");

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

    // ── 9. POST a Linear Issue webhook ─────────────────────────────────────
    let body = serde_json::to_vec(&serde_json::json!({
        "type": "Issue",
        "action": "create",
        "webhookId": "webhook-linear-001",
        "data": {
            "id": "issue-linear-123",
            "title": "Performance degradation in prod",
            "priority": 1,
            "team": { "name": "Platform" }
        }
    }))
    .unwrap();

    let sig = compute_linear_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{linear_port}/webhook"))
        .header("linear-signature", sig)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("POST to Linear webhook server failed");

    assert_eq!(
        resp.status(),
        200,
        "Linear webhook server must return 200 for a valid signed request"
    );

    // ── 10. Assert: mock Anthropic received the REAL key ──────────────────
    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Mock Anthropic was not called within 20 s — the full 5-piece pipeline \
         (webhook → linear → JetStream → agent → proxy → worker → mock) may not be wired correctly"
    );

    anthropic_mock.assert_async().await;
}

/// Full 7-stage pipeline: HTTP webhook → source → JetStream → agent → mock
/// Anthropic → RunRecord persisted → GET /runs returns the record.
///
/// This is the only test that closes both ends of the pipeline: it starts
/// from a real signed HTTP webhook (stage 1) and asserts the resulting
/// RunRecord is accessible via the HTTP management API (stage 7).
///
/// Uses a direct mock for Anthropic (no proxy/worker) to keep the test
/// focused on the webhook → /runs gap rather than re-testing key detokenization.
#[tokio::test]
async fn github_webhook_produces_run_record_accessible_via_api() {
    use async_nats::jetstream;

    // ── 1. Start NATS ──────────────────────────────────────────────────────
    let (_nats_container, nats_port) = start_nats().await;

    // ── 2. Mock Anthropic — direct (no proxy/worker needed) ───────────────
    let mock_server = MockServer::start_async().await;
    mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"LGTM from automation."}]}"#);
        })
        .await;

    // ── 3. NATS + JetStream ────────────────────────────────────────────────
    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("connect NATS");
    let js = jetstream::new(nats.clone());

    // ── 4. Create required streams ─────────────────────────────────────────
    for (name, subject) in [
        ("GITHUB", "github.>"),
        ("LINEAR", "linear.Issue.>"),
        ("CRON_TICKS", "cron.>"),
    ] {
        js.get_or_create_stream(jetstream::stream::Config {
            name: name.to_string(),
            subjects: vec![subject.to_string()],
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("create stream {name}: {e}"));
    }

    // ── 5. Seed a matching automation ──────────────────────────────────────
    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&Automation {
            id: "auto-webhook-e2e".to_string(),
            tenant_id: "default".to_string(),
            name: "Webhook E2E Test Automation".to_string(),
            trigger: "github.pull_request".to_string(),
            prompt: "WEBHOOK_E2E_PROMPT".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            mcp_servers: vec![],
            enabled: true,
            visibility: trogon_automations::Visibility::Private,
            variables: std::collections::HashMap::new(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .await
        .expect("put automation");

    // ── 6. Start trogon-github webhook server ──────────────────────────────
    let github_port = next_port();
    let webhook_secret = "webhook-e2e-secret";

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("GITHUB_WEBHOOK_PORT", github_port.to_string());
    env.set("GITHUB_WEBHOOK_SECRET", webhook_secret);
    let github_config = GithubConfig::from_env(&env);

    let nats_for_github = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        github_serve(github_config, nats_for_github)
            .await
            .expect("GitHub server error");
    });
    wait_for_port(github_port).await;

    // ── 7. Start agent runner with a real api_port ─────────────────────────
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: mock_server.base_url(),
        anthropic_token: "test-token".to_string(),
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
        api_port,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    // Wait for the agent's HTTP API to come up.
    let client = reqwest::Client::new();
    let runs_url = format!("http://127.0.0.1:{api_port}/runs");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
        {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("Agent API never came up: {e}"),
        }
    }

    // ── 8. POST a signed GitHub pull_request webhook ───────────────────────
    let body = serde_json::to_vec(&serde_json::json!({
        "action": "opened",
        "number": 99,
        "repository": { "name": "api", "owner": { "login": "acme" } },
        "pull_request": { "title": "Full E2E test PR" }
    }))
    .unwrap();
    let sig = compute_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{github_port}/webhook"))
        .header("X-GitHub-Event", "pull_request")
        .header("X-GitHub-Delivery", "delivery-e2e-001")
        .header("X-Hub-Signature-256", sig)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("POST to GitHub webhook server failed");

    assert_eq!(resp.status(), 200, "webhook server must accept signed request");

    // ── 9. Poll GET /runs until the RunRecord appears ──────────────────────
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let runs = loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .expect("GET /runs")
            .json()
            .await
            .expect("json");
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "No RunRecord appeared in GET /runs within 20 s — full pipeline \
                 (webhook → NATS → agent → Anthropic → RunRecord → /runs) may be broken"
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    // ── 10. Assert RunRecord fields ────────────────────────────────────────
    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 1, "exactly one run must be recorded");
    assert_eq!(arr[0]["automation_id"], "auto-webhook-e2e");
    assert_eq!(arr[0]["status"], "success");
    assert_eq!(arr[0]["tenant_id"], "default");
    assert_eq!(arr[0]["nats_subject"], "github.pull_request");
    assert!(
        arr[0]["output"]
            .as_str()
            .unwrap_or("")
            .contains("LGTM from automation."),
        "RunRecord output must contain the model's final text"
    );

    // ── 11. Assert GET /stats reflects the run ─────────────────────────────
    let stats: serde_json::Value = client
        .get(format!("http://127.0.0.1:{api_port}/stats"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats["total"], 1);
    assert_eq!(stats["successful_7d"], 1);
    assert_eq!(stats["failed_7d"], 0);
}

/// Full 7-stage pipeline via the Linear webhook server:
/// POST /webhook (HMAC-SHA256 linear-signature) → trogon-linear → JetStream
/// → agent → mock Anthropic → RunRecord → GET /runs returns the record.
#[tokio::test]
async fn linear_webhook_produces_run_record_accessible_via_api() {
    use async_nats::jetstream;

    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Triaged via Linear webhook."}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("connect NATS");
    let js = jetstream::new(nats.clone());

    for (name, subject) in [
        ("GITHUB", "github.>"),
        ("LINEAR", "linear.Issue.>"),
        ("CRON_TICKS", "cron.>"),
    ] {
        js.get_or_create_stream(jetstream::stream::Config {
            name: name.to_string(),
            subjects: vec![subject.to_string()],
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("create stream {name}: {e}"));
    }

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&Automation {
            id: "auto-linear-e2e".to_string(),
            tenant_id: "default".to_string(),
            name: "Linear Webhook E2E Automation".to_string(),
            trigger: "linear.Issue".to_string(),
            prompt: "LINEAR_E2E_PROMPT".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            mcp_servers: vec![],
            enabled: true,
            visibility: trogon_automations::Visibility::Private,
            variables: std::collections::HashMap::new(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .await
        .expect("put automation");

    let linear_port = next_port();
    let webhook_secret = "linear-e2e-secret";

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("LINEAR_WEBHOOK_PORT", linear_port.to_string());
    env.set("LINEAR_WEBHOOK_SECRET", webhook_secret);
    env.set("LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS", "0");
    let linear_config = LinearConfig::from_env(&env);

    let nats_for_linear = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        linear_serve(linear_config, nats_for_linear)
            .await
            .expect("Linear server error");
    });
    wait_for_port(linear_port).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: mock_server.base_url(),
        anthropic_token: "test-token".to_string(),
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
        api_port,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    let client = reqwest::Client::new();
    let runs_url = format!("http://127.0.0.1:{api_port}/runs");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
        {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("Agent API never came up: {e}"),
        }
    }

    let body = serde_json::to_vec(&serde_json::json!({
        "type": "Issue",
        "action": "create",
        "webhookId": "webhook-linear-e2e-001",
        "data": {
            "id": "issue-e2e-42",
            "title": "Production incident — DB latency spike",
            "priority": 1,
            "team": { "name": "Platform" }
        }
    }))
    .unwrap();
    let sig = compute_linear_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{linear_port}/webhook"))
        .header("linear-signature", sig)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("POST to Linear webhook server failed");

    assert_eq!(resp.status(), 200, "webhook must accept signed request");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let runs = loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .expect("GET /runs")
            .json()
            .await
            .expect("json");
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "No RunRecord appeared in GET /runs within 20 s \
                 (Linear webhook → NATS → agent → Anthropic → /runs)"
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["automation_id"], "auto-linear-e2e");
    assert_eq!(arr[0]["status"], "success");
    assert_eq!(arr[0]["tenant_id"], "default");
    assert_eq!(arr[0]["nats_subject"], "linear.Issue");
    assert!(
        arr[0]["output"]
            .as_str()
            .unwrap_or("")
            .contains("Triaged via Linear webhook."),
        "RunRecord output must contain the model's final text"
    );

    let stats: serde_json::Value = client
        .get(format!("http://127.0.0.1:{api_port}/stats"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats["total"], 1);
    assert_eq!(stats["successful_7d"], 1);
    assert_eq!(stats["failed_7d"], 0);
}

/// When the Anthropic API returns a non-retryable 4xx error for a webhook-triggered
/// run, the agent records a RunRecord with status `"failed"` that is accessible
/// via GET /runs and reflected in GET /stats as `failed_7d: 1`.
#[tokio::test]
async fn github_webhook_failed_run_appears_in_runs_api_as_failed() {
    use async_nats::jetstream;

    let (_nats_container, nats_port) = start_nats().await;

    // Anthropic returns 400 Bad Request — non-retryable, marks PermanentFailed.
    let mock_server = MockServer::start_async().await;
    mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(400)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"invalid_request_error","message":"bad request"}}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .expect("connect NATS");
    let js = jetstream::new(nats.clone());

    for (name, subject) in [
        ("GITHUB", "github.>"),
        ("LINEAR", "linear.Issue.>"),
        ("CRON_TICKS", "cron.>"),
    ] {
        js.get_or_create_stream(jetstream::stream::Config {
            name: name.to_string(),
            subjects: vec![subject.to_string()],
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("create stream {name}: {e}"));
    }

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&Automation {
            id: "auto-fail-e2e".to_string(),
            tenant_id: "default".to_string(),
            name: "Failing E2E Automation".to_string(),
            trigger: "github.pull_request".to_string(),
            prompt: "FAIL_E2E_PROMPT".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            mcp_servers: vec![],
            enabled: true,
            visibility: trogon_automations::Visibility::Private,
            variables: std::collections::HashMap::new(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .await
        .expect("put automation");

    let github_port = next_port();
    let webhook_secret = "fail-e2e-secret";

    let env = InMemoryEnv::new();
    env.set("NATS_URL", format!("localhost:{nats_port}"));
    env.set("GITHUB_WEBHOOK_PORT", github_port.to_string());
    env.set("GITHUB_WEBHOOK_SECRET", webhook_secret);
    let github_config = GithubConfig::from_env(&env);

    let nats_for_github = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        github_serve(github_config, nats_for_github)
            .await
            .expect("GitHub server error");
    });
    wait_for_port(github_port).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: mock_server.base_url(),
        anthropic_token: "test-token".to_string(),
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
        api_port,
        tenant_id: "default".to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    let client = reqwest::Client::new();
    let runs_url = format!("http://127.0.0.1:{api_port}/runs");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
        {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("Agent API never came up: {e}"),
        }
    }

    let body = serde_json::to_vec(&serde_json::json!({
        "action": "opened",
        "number": 7,
        "repository": { "name": "api", "owner": { "login": "acme" } },
        "pull_request": { "title": "PR that will fail" }
    }))
    .unwrap();
    let sig = compute_sig(webhook_secret, &body);

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{github_port}/webhook"))
        .header("X-GitHub-Event", "pull_request")
        .header("X-GitHub-Delivery", "delivery-fail-e2e")
        .header("X-Hub-Signature-256", sig)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("POST to GitHub webhook server failed");

    assert_eq!(resp.status(), 200, "webhook must accept the signed request");

    // Poll until a RunRecord with status "failed" appears.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let runs = loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .expect("GET /runs")
            .json()
            .await
            .expect("json");
        let has_failed = body
            .as_array()
            .map(|a| a.iter().any(|r| r["status"] == "failed"))
            .unwrap_or(false);
        if has_failed {
            break body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "No failed RunRecord appeared in GET /runs within 20 s — \
                 4xx Anthropic error must produce a 'failed' RunRecord accessible via API"
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["automation_id"], "auto-fail-e2e");
    assert_eq!(arr[0]["status"], "failed");
    assert_eq!(arr[0]["tenant_id"], "default");

    let stats: serde_json::Value = client
        .get(format!("http://127.0.0.1:{api_port}/stats"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats["total"], 1);
    assert_eq!(stats["successful_7d"], 0);
    assert_eq!(stats["failed_7d"], 1);
}
