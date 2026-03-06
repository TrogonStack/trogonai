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
use trogon_github::{GithubConfig, serve as github_serve};
use trogon_linear::{LinearConfig, serve as linear_serve};
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_secret_proxy::{proxy::{ProxyState, router}, stream, subjects, worker};
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
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"LGTM."}]}"#,
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

    // The agent runner requires BOTH GITHUB and LINEAR streams to exist.
    // The GitHub server created GITHUB above; create LINEAR manually so
    // runner.rs's `get_stream("LINEAR")` doesn't fail.
    js.get_or_create_stream(jetstream::stream::Config {
        name: "LINEAR".to_string(),
        subjects: vec!["linear.Issue.>".to_string()],
        ..Default::default()
    })
    .await
    .expect("Failed to create LINEAR stream");

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
        model: "claude-opus-4-6".to_string(),
        max_iterations: 1,
        github_stream_name: None,
        linear_stream_name: None,
        memory_owner: None,
        memory_repo: None,
        mcp_servers: vec![],
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

    // The agent runner requires BOTH GITHUB and LINEAR streams to exist.
    // The Linear server created LINEAR above; create GITHUB manually so
    // runner.rs's `get_stream("GITHUB")` doesn't fail.
    js.get_or_create_stream(jetstream::stream::Config {
        name: "GITHUB".to_string(),
        subjects: vec!["github.pull_request".to_string()],
        ..Default::default()
    })
    .await
    .expect("Failed to create GITHUB stream");

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
        model: "claude-opus-4-6".to_string(),
        max_iterations: 1,
        github_stream_name: None,
        linear_stream_name: None,
        memory_owner: None,
        memory_repo: None,
        mcp_servers: vec![],
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
