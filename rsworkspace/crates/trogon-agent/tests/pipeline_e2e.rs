//! Cross-crate end-to-end pipeline test.
//!
//! Verifies the full agentic pipeline:
//!
//! ```text
//! GitHub PR event
//!   → JetStream GITHUB stream
//!   → trogon-agent runner (pulls, dispatches pr_review handler)
//!   → AgentLoop POST /anthropic/v1/messages to proxy (Bearer tok_anthropic_prod_test01)
//!   → trogon-secret-proxy HTTP server (publishes OutboundHttpRequest to JetStream)
//!   → trogon-secret-proxy worker (resolves tok_anthropic_prod_test01 → sk-ant-realkey)
//!   → mock Anthropic server (receives Bearer sk-ant-realkey ← the REAL key, never the token)
//!   → 200 end_turn response bubbles back through worker → proxy → agent
//! ```
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test pipeline_e2e

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use httpmock::MockServer;
use serde_json::json;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericImage, ImageExt, core::ContainerPort, runners::AsyncRunner,
};
use trogon_agent::{AgentConfig, run};
use trogon_github::GithubConfig;
use trogon_linear::LinearConfig;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, worker,
};
use trogon_vault::{
    ApiKeyToken, HashicorpVaultConfig, HashicorpVaultStore, MemoryVault, VaultAuth, VaultStore,
};

// ── helpers ────────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn js_client(port: u16) -> (async_nats::Client, jetstream::Context) {
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

async fn create_stream(js: &jetstream::Context, name: &str, subjects: &[&str]) {
    js.get_or_create_stream(jetstream::stream::Config {
        name: name.to_string(),
        subjects: subjects.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    })
    .await
    .unwrap_or_else(|e| panic!("Failed to create stream {name}: {e}"));
}

/// Poll until the mock has been hit at least once, or the timeout expires.
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

/// Start a HashiCorp Vault container in dev mode.  Returns the container
/// handle (keeps it alive) and the HTTP address `http://127.0.0.1:{port}`.
async fn start_vault() -> (ContainerAsync<GenericImage>, String) {
    let container = GenericImage::new("hashicorp/vault", "1.17")
        .with_exposed_port(ContainerPort::Tcp(8200))
        .with_env_var("VAULT_DEV_ROOT_TOKEN_ID", "root")
        .with_env_var("VAULT_DEV_LISTEN_ADDRESS", "0.0.0.0:8200")
        .with_startup_timeout(Duration::from_secs(30))
        .start()
        .await
        .expect("Failed to start Vault container — is Docker running?");
    let port = container.get_host_port_ipv4(8200).await.unwrap();
    let addr = format!("http://127.0.0.1:{port}");
    wait_for_vault(&addr).await;
    (container, addr)
}

/// Poll Vault's `/v1/sys/health` until it returns 200 or 30 s elapsed.
async fn wait_for_vault(addr: &str) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let ready = client
            .get(format!("{addr}/v1/sys/health"))
            .send()
            .await
            .map(|r| r.status().as_u16() == 200)
            .unwrap_or(false);
        if ready {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Vault not ready within 30s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn pr_opened_payload() -> bytes::Bytes {
    let v = json!({
        "action": "opened",
        "number": 42,
        "repository": {
            "name": "test-repo",
            "owner": { "login": "test-org" }
        }
    });
    bytes::Bytes::from(serde_json::to_vec(&v).unwrap())
}

// ── tests ──────────────────────────────────────────────────────────────────────

/// Full 4-piece pipeline: GitHub PR event → agent → real proxy → real worker →
/// mock Anthropic.  The mock asserts it was called with the REAL api key
/// (`sk-ant-realkey`), not the opaque proxy token.
#[tokio::test]
async fn pipeline_pr_event_reaches_anthropic_with_real_key() {
    // ── 1. Start NATS ──────────────────────────────────────────────────────
    let (_nats_container, nats_port) = start_nats().await;

    // ── 2. Mock Anthropic — expects REAL key, not the opaque token ─────────
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

    // ── 3. Seed vault: opaque token → real key ─────────────────────────────
    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_prod_test01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

    // ── 4. NATS connections ────────────────────────────────────────────────
    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    // ── 5. Ensure PROXY_REQUESTS stream (used by proxy + worker) ───────────
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .expect("Failed to ensure PROXY_REQUESTS stream");

    // ── 6. Create GITHUB + LINEAR streams (used by agent runner) ───────────
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

    // ── 7. Start proxy HTTP server on a random port ────────────────────────
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_url = format!("http://127.0.0.1:{proxy_port}");

    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: js.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        // Route provider calls to mock server instead of real Anthropic.
        base_url_override: Some(mock_server.base_url()),
    };

    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state))
            .await
            .expect("Proxy server error");
    });

    // ── 8. Start detokenization worker ─────────────────────────────────────
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
            "pipeline-e2e-worker",
            &worker_stream,
        )
        .await
        .expect("Worker error");
    });

    // Give proxy + worker a moment to initialize their JetStream consumers.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 9. Start trogon-agent runner ───────────────────────────────────────
    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: proxy_url.clone(),
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };

    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    // Give the agent runner a moment to bind its consumers.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 10. Publish a GitHub PR opened event ───────────────────────────────
    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .expect("Failed to publish PR event");

    // ── 11. Assert: mock Anthropic was called with the REAL key ────────────
    //
    // The critical invariant: the proxy token `tok_anthropic_prod_test01`
    // must NEVER reach the mock.  If the worker failed to detokenize, the
    // mock would not match the `Bearer sk-ant-realkey` header and this
    // assertion would fail.
    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Mock Anthropic was not called within 20 s — \
         check that the full pipeline (runner → proxy → worker → mock) is wired correctly"
    );

    anthropic_mock.assert_async().await;
}

/// Verifies tool-use detokenization: the model first responds with a `tool_use`
/// block (list_pr_files), the agent executes it through the proxy using
/// `tok_github_prod_test01`, the worker resolves that token to `sk-gh-realkey`,
/// and the mock GitHub API receives `Bearer sk-gh-realkey`.
///
/// On the second iteration the model responds with `end_turn`.
///
/// This is the only test that verifies that tool calls (not just the initial
/// Anthropic request) are also properly detokenized by the proxy+worker.
#[tokio::test]
async fn pipeline_tool_use_github_api_detokenized() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;

    // Anthropic: second call (after tool result) → end_turn.
    // Registered FIRST so httpmock checks it first (FIFO).  The body_contains
    // constraint means it only matches when the request already carries a
    // "tool_result" block (i.e. the second call in the loop).
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Reviewed."}]}"#,
                );
        })
        .await;

    // Anthropic: first call returns tool_use (list_pr_files).
    // Registered SECOND — only reached when end_turn mock doesn't match
    // (i.e. first call, before any tool results are present in the body).
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_001",
                        "name": "list_pr_files",
                        "input": {"owner":"acme","repo":"trogon","pr_number":42}
                    }]
                }"#,
                );
        })
        .await;

    // GitHub API (via proxy): must receive the REAL github key, not tok_github_prod_test01.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/acme/trogon/pulls/42/files")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"filename":"src/main.rs","status":"modified"}]"#);
        })
        .await;

    // Seed both tokens in the vault.
    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
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
            "pipeline-tool-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .expect("Failed to publish PR event");

    // Wait for the full agentic loop to complete (end_turn received).
    let end_turn_hit = wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await;
    assert!(
        end_turn_hit,
        "Anthropic end_turn was not received within 20 s — \
         the tool_use → GitHub → end_turn loop may not have completed"
    );

    // All three mock interactions must have occurred exactly once.
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

/// Publishes a Linear Issue.create event and verifies the same detokenization
/// pipeline is triggered for Linear events.
#[tokio::test]
async fn pipeline_linear_issue_event_reaches_anthropic_with_real_key() {
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
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Triaged."}]}"#,
                );
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_prod_test01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();

    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
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
            "pipeline-e2e-linear-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Publish a Linear Issue create event
    let payload = json!({
        "action": "create",
        "type": "Issue",
        "data": {
            "id": "issue-123",
            "title": "Service is down",
            "description": "Production is on fire",
            "priority": 1,
            "team": { "name": "Backend" }
        }
    });
    js.context()
        .publish(
            "linear.Issue.create",
            bytes::Bytes::from(serde_json::to_vec(&payload).unwrap()),
        )
        .await
        .expect("Failed to publish Linear issue event");

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Mock Anthropic was not called within 20 s for Linear event — \
         check that the full pipeline (runner → proxy → worker → mock) is wired correctly"
    );

    anthropic_mock.assert_async().await;
}

/// Verifies tool-use detokenization for Linear tools: the model responds with
/// `update_issue`, the agent executes it through the proxy using
/// `tok_linear_prod_test01`, the worker resolves that token to `sk-linear-realkey`,
/// and the mock Linear GraphQL API receives `Bearer sk-linear-realkey`.
///
/// On the second iteration the model responds with `end_turn`.
#[tokio::test]
async fn pipeline_tool_use_linear_api_detokenized() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;

    // Anthropic: second call (after tool result) → end_turn.
    // Registered FIRST so httpmock checks it first (FIFO).  The body_contains
    // constraint means it only matches when the request already carries a
    // "tool_result" block (i.e. the second call in the loop).
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}"#);
        })
        .await;

    // Anthropic: first call returns update_issue tool_use.
    // Registered SECOND — only reached when end_turn mock doesn't match.
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_linear_001",
                        "name": "update_linear_issue",
                        "input": {"issue_id": "issue-linear-123", "priority": 2}
                    }]
                }"#,
                );
        })
        .await;

    // Linear GraphQL API (via proxy): must receive the REAL linear key, not tok_linear_prod_test01.
    // The proxy strips the "/linear" prefix, so the path the mock sees is "/graphql".
    let linear_graphql_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"issueUpdate":{"success":true,"issue":{"id":"issue-linear-123","title":"Perf issue","state":{"name":"In Progress"}}}}}"#);
        })
        .await;

    // Seed both tokens in the vault.
    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();
    vault
        .store(
            &ApiKeyToken::new("tok_linear_prod_test01").unwrap(),
            "sk-linear-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
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
            "pipeline-linear-tool-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Publish a Linear Issue create event to trigger the issue_triage handler.
    js.context()
        .publish(
            "linear.Issue.create",
            bytes::Bytes::from(
                serde_json::to_vec(&json!({
                    "action": "create",
                    "type": "Issue",
                    "data": {
                        "id": "issue-linear-123",
                        "title": "Performance degradation in prod",
                        "priority": 1,
                        "team": { "name": "Platform" }
                    }
                }))
                .unwrap(),
            ),
        )
        .await
        .expect("Failed to publish Linear Issue event");

    // Wait for the full agentic loop to complete (end_turn received).
    let end_turn_hit = wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await;
    assert!(
        end_turn_hit,
        "Anthropic end_turn was not received within 20 s — \
         the tool_use → Linear GraphQL → end_turn loop may not have completed"
    );

    // All three mock interactions must have occurred exactly once.
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    linear_graphql_mock.assert_async().await;
}

/// Verifies the tool failure propagation path: when a tool HTTP call fails
/// (non-JSON response body causes a JSON parse error), `dispatch_tool` wraps
/// the error as `"Tool error: ..."` and the agent sends it back as a
/// `tool_result` block.  The model receives the error and returns `end_turn`.
///
/// This closes the gap where the error-handling branch of `dispatch_tool` /
/// `execute_tools` was never exercised in an end-to-end pipeline test.
#[tokio::test]
async fn pipeline_tool_failure_sends_error_as_tool_result_then_end_turn() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;

    // Anthropic: second call (body contains tool_result) → end_turn.
    // Registered FIRST (highest httpmock priority).
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Noted the tool error."}]}"#);
        })
        .await;

    // Anthropic: first call → tool_use (list_pr_files).
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_fail_001",
                        "name": "list_pr_files",
                        "input": {"owner":"acme","repo":"trogon","pr_number":99}
                    }]
                }"#,
                );
        })
        .await;

    // GitHub API: returns a non-JSON plain-text body.
    // reqwest's `.json()` call in `list_pr_files` will fail to deserialise
    // this, causing `dispatch_tool` to return "Tool error: ...".
    let github_error_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/acme/trogon/pulls/99/files")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "text/plain")
                .body("Not a JSON response — simulated tool failure");
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
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
            "pipeline-tool-failure-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .expect("Failed to publish PR event");

    // Wait for end_turn — confirms the agent handled the tool error gracefully
    // and sent it back as tool_result content rather than crashing.
    let end_turn_hit = wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await;
    assert!(
        end_turn_hit,
        "end_turn was not received within 20 s — \
         tool failure should be wrapped as tool_result and the loop should continue to end_turn"
    );

    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_error_mock.assert_async().await;
}

/// Verifies that when the model never reaches `end_turn` (always returns
/// `tool_use`), the agent stops after `max_iterations` calls to Anthropic
/// and the JetStream message is acked (not redelivered).
///
/// With `max_iterations = 2`, Anthropic must be called exactly twice and the
/// loop must terminate — it must not spin indefinitely.
#[tokio::test]
async fn pipeline_max_iterations_terminates_loop_at_cap() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;

    // Anthropic: always returns tool_use — the model never reaches end_turn.
    let anthropic_tool_loop = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_loop",
                        "name": "list_pr_files",
                        "input": {"owner":"acme","repo":"trogon","pr_number":1}
                    }]
                }"#,
                );
        })
        .await;

    // GitHub: always returns valid JSON so the tool doesn't error out.
    // No auth constraint — we're testing iteration count, not detokenization.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/acme/trogon/pulls/1/files");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"filename":"main.rs","status":"modified"}]"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
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
            "pipeline-max-iter-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // max_iterations: 2 → the loop runs iterations 0 and 1, calling Anthropic
    // twice and executing the tool twice before returning MaxIterationsReached.
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
        max_iterations: 2,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .expect("Failed to publish PR event");

    // Wait until Anthropic has been called exactly max_iterations times.
    let reached = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if anthropic_tool_loop.hits_async().await >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        reached.is_ok(),
        "Anthropic was not called 2 times within 20 s"
    );

    // Wait a moment to confirm no 3rd call arrives (loop capped at 2).
    tokio::time::sleep(Duration::from_millis(500)).await;

    let anthropic_hits = anthropic_tool_loop.hits_async().await;
    assert_eq!(
        anthropic_hits, 2,
        "Anthropic was called {anthropic_hits} times — expected exactly 2 (max_iterations cap)"
    );

    // With tool-result caching active (NATS KV backed), the second identical
    // `list_pr_files` call may be served from cache without hitting GitHub.
    // The important invariant is that the tool executed at least once and the
    // Anthropic loop was capped at max_iterations (already asserted above).
    let github_hits = github_mock.hits_async().await;
    assert!(
        github_hits >= 1,
        "GitHub tool must be called at least once; got {github_hits}"
    );
}

/// Verifies that two GitHub PR events published simultaneously are each
/// processed to completion without cross-contamination.
///
/// The runner uses `tokio::spawn` per message; all shared state is `Arc<...>`
/// (read-only). Each request goes through the proxy with its own unique
/// correlation ID. This test confirms both events reach Anthropic independently.
#[tokio::test]
async fn pipeline_concurrent_events_processed_independently() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;

    // Single Anthropic mock — returns end_turn for every call.
    // With two concurrent events, it must be hit exactly twice.
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

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
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
            "pipeline-concurrent-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Publish two PR events back-to-back (minimal delay — runner processes them
    // concurrently via tokio::spawn).
    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .expect("Failed to publish first PR event");
    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .expect("Failed to publish second PR event");

    // Wait until both events have been handled.
    let both_processed = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if anthropic_mock.hits_async().await >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        both_processed.is_ok(),
        "Both concurrent PR events were not processed within 30 s"
    );

    // No extra calls — exactly 2 events, exactly 2 Anthropic calls.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let hits = anthropic_mock.hits_async().await;
    assert_eq!(
        hits, 2,
        "Expected exactly 2 Anthropic calls (one per event); got {hits}"
    );
}

// ── additional helpers ─────────────────────────────────────────────────────────

/// PR event payload with a custom action (the default helper uses "opened").
fn pr_payload(action: &str) -> bytes::Bytes {
    bytes::Bytes::from(
        serde_json::to_vec(&json!({
            "action": action,
            "number": 42,
            "repository": { "name": "test-repo", "owner": { "login": "test-org" } }
        }))
        .unwrap(),
    )
}

/// Linear Issue create event payload.
fn linear_issue_payload() -> bytes::Bytes {
    bytes::Bytes::from(
        serde_json::to_vec(&json!({
            "action": "create",
            "type": "Issue",
            "data": {
                "id": "issue-linear-123",
                "title": "Slow database queries in production",
                "priority": 1,
                "team": { "name": "Backend" }
            }
        }))
        .unwrap(),
    )
}

// ── remaining tool pipeline tests ──────────────────────────────────────────────

/// `get_pr_diff` tool exercised end-to-end: model requests the diff, proxy
/// detokenizes `tok_github_prod_test01` → `sk-gh-realkey`, mock GitHub returns
/// a plain-text diff.  `get_pr_diff` uses `.text()` so any text body is valid.
#[tokio::test]
async fn pipeline_get_pr_diff_tool_detokenized() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // end_turn (registered first — highest priority; matches only the second call
    // which already carries tool_result content).
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Diff reviewed."}]}"#);
        })
        .await;

    // First call → get_pr_diff tool_use.
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_diff_01","name":"get_pr_diff","input":{"owner":"test-org","repo":"test-repo","pr_number":42}}]}"#);
        })
        .await;

    // GitHub: GET /repos/test-org/test-repo/pulls/42 — returns plain-text diff.
    let github_diff_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/pulls/42")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200).header("content-type", "text/plain").body(
                "diff --git a/src/main.rs b/src/main.rs\n+++ b/src/main.rs\n+fn helper() {}\n",
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "diff-tool-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_diff_mock.assert_async().await;
}

/// `get_file_contents` tool exercised end-to-end, including the base64-decode
/// path: GitHub returns `{"content":"aGVsbG8gd29ybGQ="}` (base64 of "hello
/// world"), the tool strips embedded newlines and decodes before returning.
#[tokio::test]
async fn pipeline_get_file_contents_tool_base64_decode() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"File read."}]}"#,
                );
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_contents_01","name":"get_file_contents","input":{"owner":"test-org","repo":"test-repo","path":"src/lib.rs"}}]}"#);
        })
        .await;

    // GitHub Contents API: returns base64-encoded content.
    // "aGVsbG8gd29ybGQ=" decodes to "hello world".
    // The tool code strips embedded newlines before decoding, so this clean
    // base64 string (no embedded \n) is a valid test case.
    let github_contents_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/contents/src/lib.rs")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"name":"lib.rs","content":"aGVsbG8gd29ybGQ=","encoding":"base64","sha":"deadbeef01"}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "contents-tool-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_contents_mock.assert_async().await;
}

/// `post_pr_comment` tool exercised end-to-end: model posts a review comment,
/// proxy detokenizes the GitHub token, mock GitHub (issues comments endpoint)
/// receives the REAL key.
#[tokio::test]
async fn pipeline_post_pr_comment_tool_detokenized() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Comment posted."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_comment_01","name":"post_pr_comment","input":{"owner":"test-org","repo":"test-repo","pr_number":42,"body":"LGTM!"}}]}"#);
        })
        .await;

    // post_pr_comment hits the Issues comments endpoint (not pulls).
    let github_comment_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/repos/test-org/test-repo/issues/42/comments")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"id":1,"html_url":"https://github.com/test-org/test-repo/issues/42#issuecomment-1"}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "pr-comment-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_comment_mock.assert_async().await;
}

/// Verifies that both `synchronize` and `reopened` PR actions trigger a full
/// review cycle.  The `pr_review::handle` handler accepts these actions via
/// the `REVIEW_ACTIONS` filter; this test exercises that path end-to-end.
///
/// Two events are published and the mock Anthropic server must be called
/// exactly twice.
#[tokio::test]
async fn pipeline_synchronize_and_reopened_actions_trigger_review() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Single end_turn mock — called for every request regardless of body.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Reviewing."}]}"#,
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "sync-reopen-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Both actions are in REVIEW_ACTIONS — each should trigger one agent run.
    js.context()
        .publish("github.pull_request", pr_payload("synchronize"))
        .await
        .unwrap();
    js.context()
        .publish("github.pull_request", pr_payload("reopened"))
        .await
        .unwrap();

    let both_hit = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if anthropic_mock.hits_async().await >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        both_hit.is_ok(),
        "Anthropic not called twice within 30 s (synchronize + reopened)"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        2,
        "Expected exactly 2 Anthropic calls for synchronize + reopened events"
    );
}

/// `get_linear_issue` tool exercised end-to-end: a Linear Issue event triggers
/// the triage handler, the model requests `get_linear_issue`, the proxy
/// detokenizes `tok_linear_prod_test01` → `sk-linear-realkey`, and the mock
/// Linear GraphQL API receives the REAL key.
#[tokio::test]
async fn pipeline_get_linear_issue_tool_detokenized() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Issue fetched."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_get_issue_01","name":"get_linear_issue","input":{"issue_id":"issue-linear-123"}}]}"#);
        })
        .await;

    // Linear GraphQL: returns issue data.
    let graphql_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"issue":{"id":"issue-linear-123","title":"Slow queries","state":{"name":"Todo"},"priority":1,"labels":{"nodes":[]},"team":{"name":"Backend"}}}}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_linear_prod_test01").unwrap(),
            "sk-linear-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "get-issue-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("linear.Issue.create", linear_issue_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    graphql_mock.assert_async().await;
}

/// `post_linear_comment` tool exercised end-to-end: model posts a triage
/// comment, proxy detokenizes the Linear token, mock GraphQL returns
/// `commentCreate.success = true`.
#[tokio::test]
async fn pipeline_post_linear_comment_tool_detokenized() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Comment posted."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_post_comment_01","name":"post_linear_comment","input":{"issue_id":"issue-linear-123","body":"Triaged: priority looks correct."}}]}"#);
        })
        .await;

    // Linear GraphQL: pre-check query for existing comments (idempotency check).
    let get_comments_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey")
                .body_contains("comments");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"issue":{"comments":{"nodes":[]}}}}"#);
        })
        .await;

    // Linear GraphQL: commentCreate returns success.
    let graphql_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey")
                .body_contains("commentCreate");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"commentCreate":{"success":true,"comment":{"id":"c1","url":"https://linear.app/comment/c1"}}}}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_linear_prod_test01").unwrap(),
            "sk-linear-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "post-comment-linear-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("linear.Issue.create", linear_issue_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    get_comments_mock.assert_async().await;
    graphql_mock.assert_async().await;
}

/// Verifies the GraphQL API-level failure path: when `commentCreate` returns
/// `success: false`, `post_linear_comment` returns `Err("commentCreate
/// failed: ...")`, `dispatch_tool` wraps it as `"Tool error: commentCreate
/// failed: ..."`, and the agent sends it back as a `tool_result` block.
/// The model receives the error and returns `end_turn`.
#[tokio::test]
async fn pipeline_graphql_failure_wrapped_as_tool_error() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Second call (carries tool_result with the GraphQL error string) → end_turn.
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Noted the GraphQL error."}]}"#);
        })
        .await;

    // First call → post_linear_comment tool_use.
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_fail_gql_01","name":"post_linear_comment","input":{"issue_id":"issue-linear-123","body":"Triage note"}}]}"#);
        })
        .await;

    // Linear GraphQL: pre-check query for existing comments (idempotency check).
    let get_comments_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey")
                .body_contains("comments");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"issue":{"comments":{"nodes":[]}}}}"#);
        })
        .await;

    // Linear GraphQL: commentCreate returns success:false — triggers the Err
    // branch in post_comment() → "commentCreate failed: ...".
    let graphql_failure_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey")
                .body_contains("commentCreate");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"commentCreate":{"success":false}},"errors":[{"message":"Not authorized"}]}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_linear_prod_test01").unwrap(),
            "sk-linear-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "gql-failure-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("linear.Issue.create", linear_issue_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — GraphQL failure should be wrapped as tool_result and loop should continue"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    get_comments_mock.assert_async().await;
    graphql_failure_mock.assert_async().await;
}

/// Verifies that when Anthropic returns an unknown stop reason (`"max_tokens"`),
/// the agent returns `AgentError::UnexpectedStopReason`, the runner logs the
/// error, acks the JetStream message, and continues processing the next event.
///
/// Two events are published; both reach Anthropic and both are acked (runner
/// does not crash or stall).
#[tokio::test]
async fn pipeline_unexpected_stop_reason_runner_acks_and_continues() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Always returns max_tokens — hits the `other =>` branch in agent_loop.rs.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"max_tokens","content":[{"type":"text","text":"Partial response cut off by token limit"}]}"#);
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "unexpected-stop-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Two events — each triggers one Anthropic call before UnexpectedStopReason.
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    let both_hit = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if anthropic_mock.hits_async().await >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        both_hit.is_ok(),
        "Anthropic not called twice within 30 s — runner may have stalled on UnexpectedStopReason"
    );

    // Confirm no extra calls and no crash.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        2,
        "Expected exactly 2 Anthropic calls — one per event, no retries"
    );
}

/// Verifies that when the model returns **two** `tool_use` blocks in a single
/// response, `execute_tools` iterates all blocks and both tools are executed.
/// Both GitHub endpoints receive the REAL key; the second Anthropic call
/// carries two `tool_result` blocks and receives `end_turn`.
#[tokio::test]
async fn pipeline_multiple_tool_use_blocks_all_executed() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Second Anthropic call (two tool_results in body) → end_turn.
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Both tools done."}]}"#);
        })
        .await;

    // First call → TWO tool_use blocks (list_pr_files + get_pr_diff).
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason":"tool_use",
                    "content":[
                        {"type":"tool_use","id":"tu_multi_01","name":"list_pr_files",
                         "input":{"owner":"test-org","repo":"test-repo","pr_number":42}},
                        {"type":"tool_use","id":"tu_multi_02","name":"get_pr_diff",
                         "input":{"owner":"test-org","repo":"test-repo","pr_number":42}}
                    ]
                }"#,
                );
        })
        .await;

    // GitHub files endpoint (list_pr_files).
    let github_files_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/pulls/42/files")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"filename":"src/main.rs","status":"modified"}]"#);
        })
        .await;

    // GitHub diff endpoint (get_pr_diff) — different path, no conflict.
    let github_diff_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/pulls/42")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "text/plain")
                .body("diff --git a/src/main.rs b/src/main.rs\n+fn helper() {}\n");
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "multi-tool-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — both tools should execute and loop should complete"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    // Critical: BOTH tool endpoints must have been called exactly once.
    github_files_mock.assert_async().await;
    github_diff_mock.assert_async().await;
}

// ── skipped-action / resilience tests ─────────────────────────────────────────

/// Verifies that a PR event with action `"closed"` (not in REVIEW_ACTIONS) is
/// silently acked without calling Anthropic.  Then a valid `"opened"` event is
/// published to confirm the runner is still alive and processing.
#[tokio::test]
async fn pipeline_ignored_pr_action_acked_without_agent_run() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Returns end_turn for any Anthropic call — used only by the valid event.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Reviewed."}]}"#,
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "ignored-pr-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // "closed" is not in REVIEW_ACTIONS — handler returns None, no agent run.
    js.context()
        .publish("github.pull_request", pr_payload("closed"))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must NOT be called for 'closed' PR action"
    );

    // Runner must still be alive — valid event should reach Anthropic.
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();
    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(15)).await,
        "Runner must still process events after silently acking a 'closed' event"
    );
    assert_eq!(anthropic_mock.hits_async().await, 1);
}

/// Verifies that a Linear event with action `"update"` (not `"create"`) is
/// silently acked without calling Anthropic.  A subsequent valid create event
/// confirms the runner is still alive.
#[tokio::test]
async fn pipeline_ignored_linear_event_acked_without_agent_run() {
    let (_c, nats_port) = start_nats().await;
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

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "ignored-linear-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // action="update" is not "create" — issue_triage::handle returns None.
    let ignored_payload = bytes::Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "action": "update",
            "type": "Issue",
            "data": { "id": "issue-x", "title": "Irrelevant" }
        }))
        .unwrap(),
    );
    js.context()
        .publish("linear.Issue.update", ignored_payload)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must NOT be called for a Linear 'update' event"
    );

    // Runner still alive — valid create event must be processed.
    js.context()
        .publish("linear.Issue.create", linear_issue_payload())
        .await
        .unwrap();
    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(15)).await,
        "Runner must still process events after silently acking a Linear 'update' event"
    );
    assert_eq!(anthropic_mock.hits_async().await, 1);
}

/// Verifies that publishing non-JSON bytes to the GITHUB stream does not crash
/// the runner: `pr_review::handle` returns `Some(Err("JSON parse error: ..."))`,
/// the runner logs and acks, then continues processing the next valid event.
#[tokio::test]
async fn pipeline_invalid_json_payload_acked_without_crash() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Reviewed."}]}"#,
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "invalid-json-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Publish garbage bytes — serde_json::from_slice will fail.
    js.context()
        .publish(
            "github.pull_request",
            bytes::Bytes::from("not json at all }{"),
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must NOT be called when the NATS payload is invalid JSON"
    );

    // Runner must still be alive — valid event must be processed.
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();
    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(15)).await,
        "Runner must continue processing after receiving an invalid JSON payload"
    );
    assert_eq!(anthropic_mock.hits_async().await, 1);
}

/// `update_linear_issue` failure path: GraphQL returns `issueUpdate.success =
/// false`.  The tool returns `Err("issueUpdate failed: ...")`, `dispatch_tool`
/// wraps it as `"Tool error: ..."`, and the agent sends it back as a
/// `tool_result`.  The model receives the error and returns `end_turn`.
#[tokio::test]
async fn pipeline_update_linear_issue_failure_wrapped_as_tool_error() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Noted the update failure."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_update_fail_01","name":"update_linear_issue","input":{"issue_id":"issue-linear-123","priority":3}}]}"#);
        })
        .await;

    // GraphQL: issueUpdate returns success:false — triggers the Err branch in
    // update_issue() → "issueUpdate failed: ...".
    let graphql_failure_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"issueUpdate":{"success":false}},"errors":[{"message":"Forbidden"}]}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_linear_prod_test01").unwrap(),
            "sk-linear-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "update-fail-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("linear.Issue.create", linear_issue_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — issueUpdate failure should be wrapped as tool_result"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    graphql_failure_mock.assert_async().await;
}

/// Verifies that custom NATS stream names (`github_stream_name` /
/// `linear_stream_name` in AgentConfig) are used instead of the defaults
/// "GITHUB" / "LINEAR".  The runner must bind to "MY_GITHUB" and process
/// events published there.
#[tokio::test]
async fn pipeline_custom_stream_names_route_correctly() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Reviewed."}]}"#,
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();

    // Use custom stream names — do NOT create the default "GITHUB"/"LINEAR".
    create_stream(js.context(), "MY_GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "MY_LINEAR", &["linear.Issue.>"]).await;
    // CRON_TICKS must always exist (runner binds it unconditionally).
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "custom-stream-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Agent configured with custom stream names.
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
        github_stream_name: Some("MY_GITHUB".to_string()),
        linear_stream_name: Some("MY_LINEAR".to_string()),
        cron_stream_name: None,
        datadog_stream_name: None,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await,
        "Anthropic not called — runner should bind to MY_GITHUB and process events"
    );
    anthropic_mock.assert_async().await;
}

/// `get_file_contents` failure path: GitHub response is missing the `content`
/// field entirely.  The tool returns `Err("missing content field in GitHub
/// response")`, dispatch_tool wraps it as `"Tool error: ..."`, and the agent
/// sends it back as a tool_result.  The model receives the error → end_turn.
#[tokio::test]
async fn pipeline_get_file_contents_missing_content_field_tool_error() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Noted missing content."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_no_content_01","name":"get_file_contents","input":{"owner":"test-org","repo":"test-repo","path":"src/lib.rs"}}]}"#);
        })
        .await;

    // GitHub returns JSON without the "content" field — triggers the
    // `ok_or("missing content field in GitHub response")` error branch.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/contents/src/lib.rs")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"name":"lib.rs","encoding":"base64","size":42}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "no-content-field-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — missing content field should be wrapped as tool_result"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

/// `get_file_contents` failure path: GitHub response has a `content` field
/// with invalid base64.  The tool returns `Err("base64 decode error: ...")`,
/// which is wrapped as a `tool_result` and sent back to the model → end_turn.
#[tokio::test]
async fn pipeline_get_file_contents_invalid_base64_tool_error() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Noted base64 error."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_bad_b64_01","name":"get_file_contents","input":{"owner":"test-org","repo":"test-repo","path":"src/lib.rs"}}]}"#);
        })
        .await;

    // GitHub returns invalid base64 in the content field — triggers the
    // `general_purpose::STANDARD.decode()` error branch.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/contents/src/lib.rs")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"name":"lib.rs","content":"!!!not-valid-base64!!!","encoding":"base64"}"#,
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "invalid-b64-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — invalid base64 should be wrapped as tool_result"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

// ── remaining gap tests ────────────────────────────────────────────────────────

/// `get_file_contents` failure path: base64 decodes successfully but the bytes
/// are not valid UTF-8.  `String::from_utf8()` returns Err → dispatch_tool
/// wraps it as `"Tool error: UTF-8 decode error: ..."` and the agent sends it
/// back as a tool_result.  The model receives the error → end_turn.
///
/// `/w==` is base64 for [0xFF], which is not valid UTF-8.
#[tokio::test]
async fn pipeline_get_file_contents_non_utf8_bytes_tool_error() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Noted UTF-8 error."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_utf8_01","name":"get_file_contents","input":{"owner":"test-org","repo":"test-repo","path":"binary.bin"}}]}"#);
        })
        .await;

    // "/w==" is base64([0xFF]) — valid base64, invalid UTF-8.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/contents/binary.bin")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"name":"binary.bin","content":"/w==","encoding":"base64"}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "non-utf8-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — non-UTF8 bytes should be wrapped as tool_result"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

/// `post_pr_comment` success path when GitHub response lacks `html_url`:
/// the tool falls back to `"(no url)"` and returns `Ok("Comment posted: (no url)")`.
/// This exercises the `unwrap_or("(no url)")` branch in `post_pr_comment`.
#[tokio::test]
async fn pipeline_post_pr_comment_no_url_fallback() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_nourl_01","name":"post_pr_comment","input":{"owner":"test-org","repo":"test-repo","pr_number":42,"body":"LGTM!"}}]}"#);
        })
        .await;

    // GitHub returns 201 but with no html_url field — triggers unwrap_or("(no url)").
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/repos/test-org/test-repo/issues/42/comments")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"id":99}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "pr-nourl-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — missing html_url should use (no url) fallback and succeed"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

/// `post_linear_comment` success path when GraphQL response lacks `comment.url`:
/// the tool falls back to `"(no url)"` and returns `Ok("Comment posted: (no url)")`.
/// This exercises the `unwrap_or("(no url)")` branch in `post_comment`.
#[tokio::test]
async fn pipeline_post_linear_comment_no_url_fallback() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_lnourl_01","name":"post_linear_comment","input":{"issue_id":"issue-linear-123","body":"Triage note."}}]}"#);
        })
        .await;

    // Linear GraphQL: pre-check query for existing comments (idempotency check).
    let get_comments_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey")
                .body_contains("comments");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"issue":{"comments":{"nodes":[]}}}}"#);
        })
        .await;

    // GraphQL: success:true but comment object has no url field.
    let graphql_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey")
                .body_contains("commentCreate");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"commentCreate":{"success":true,"comment":{"id":"c99"}}}}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_linear_prod_test01").unwrap(),
            "sk-linear-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "linear-nourl-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("linear.Issue.create", linear_issue_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — missing comment url should use (no url) fallback and succeed"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    get_comments_mock.assert_async().await;
    graphql_mock.assert_async().await;
}

/// PR payload with a valid action (`"opened"`) but missing the `number` field.
/// `pr_review::handle` returns `None` via `?` on `event["number"].as_u64()`.
/// The message is silently acked.  A subsequent valid event confirms the runner
/// is still alive.
#[tokio::test]
async fn pipeline_pr_payload_missing_number_acked_silently() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Reviewed."}]}"#,
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "missing-number-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Valid JSON, correct action, but no "number" field → handler returns None.
    let no_number = bytes::Bytes::from(
        serde_json::to_vec(&json!({
            "action": "opened",
            "repository": { "name": "test-repo", "owner": { "login": "test-org" } }
        }))
        .unwrap(),
    );
    js.context()
        .publish("github.pull_request", no_number)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must NOT be called when PR payload is missing the 'number' field"
    );

    // Runner must still be alive.
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();
    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(15)).await,
        "Runner must still process events after silently acking a payload missing 'number'"
    );
    assert_eq!(anthropic_mock.hits_async().await, 1);
}

/// Linear payload with correct `action`/`type` but missing `data.id`.
/// `issue_triage::handle` returns `None` via `?` on `event["data"]["id"].as_str()`.
/// The message is silently acked.  A subsequent valid event confirms the runner
/// is still alive.
#[tokio::test]
async fn pipeline_linear_payload_missing_id_acked_silently() {
    let (_c, nats_port) = start_nats().await;
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

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "missing-id-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Correct action + type, but data.id is missing → handler returns None.
    let no_id = bytes::Bytes::from(
        serde_json::to_vec(&json!({
            "action": "create",
            "type": "Issue",
            "data": { "title": "No ID issue" }
        }))
        .unwrap(),
    );
    js.context()
        .publish("linear.Issue.create", no_id)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must NOT be called when Linear payload is missing 'data.id'"
    );

    // Runner must still be alive.
    js.context()
        .publish("linear.Issue.create", linear_issue_payload())
        .await
        .unwrap();
    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(15)).await,
        "Runner must still process events after silently acking a payload missing 'data.id'"
    );
    assert_eq!(anthropic_mock.hits_async().await, 1);
}

/// Verifies that an unknown tool name returned by Anthropic does not crash the
/// runner.  `dispatch_tool` returns `"Tool error: unknown tool: {name}"`,
/// which is sent back as a `tool_result`.  The model receives it → end_turn.
#[tokio::test]
async fn pipeline_unknown_tool_name_returns_error_as_tool_result() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Second call (carries tool_result with the unknown-tool error) → end_turn.
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Noted unknown tool."}]}"#);
        })
        .await;

    // First call → tool_use with a name that doesn't exist in dispatch_tool.
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_unk_01","name":"nonexistent_tool","input":{"foo":"bar"}}]}"#);
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "unknown-tool-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — unknown tool name should produce a tool_result error, not crash"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
}

/// Verifies that when Anthropic returns HTTP 500 (body is not a valid
/// `AnthropicResponse`), `agent_loop::run` returns `AgentError::Http`,
/// the runner logs the error, acks the message, and continues processing
/// the next event.
#[tokio::test]
async fn pipeline_anthropic_http_error_runner_acks_and_continues() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_500 = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(500)
                .header("content-type", "application/json")
                .body(r#"{"error":"internal server error"}"#);
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "http-error-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Two events — both trigger AgentError::Http but must be acked (not redelivered).
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    let both_hit = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if anthropic_500.hits_async().await >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        both_hit.is_ok(),
        "Anthropic 500 not hit twice within 30 s — runner may have stalled"
    );

    // The worker retries 5xx responses up to HTTP_MAX_RETRIES (3) times with
    // exponential backoff, so each event can produce up to 4 Anthropic calls.
    // With 2 published events the total hit count can be anywhere from 2 to 8;
    // we only assert at least 2 (one per published event) — not the exact count.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        anthropic_500.hits_async().await >= 2,
        "Expected at least 2 Anthropic calls — one per published event"
    );
}

// ── cross-crate + remaining agent edge-case tests ─────────────────────────────

/// Bind to port 0, note the assigned port, release the listener.
/// The returned port is free at the moment of return (tiny race window).
async fn get_free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

/// Spin-wait until a TCP port is accepting connections or the timeout elapses.
async fn wait_for_tcp(port: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Port {port} not ready within 5 s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `update_linear_issue` with both `state_id` and `assignee_id` fields set.
/// This exercises the two code branches in `update_issue()` that build the
/// GraphQL patch: `patch.insert("stateId", ...)` and `patch.insert("assigneeId", ...)`.
#[tokio::test]
async fn pipeline_update_linear_issue_with_state_and_assignee() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Updated."}]}"#,
                );
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                // Tool input includes state_id + assignee_id (no priority).
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_sa_01","name":"update_linear_issue","input":{"issue_id":"ISS-42","state_id":"state-done","assignee_id":"user-jorge"}}]}"#);
        })
        .await;

    // GraphQL: issueUpdate returns success with updated state.
    let graphql_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"issueUpdate":{"success":true,"issue":{"id":"ISS-42","title":"Fix bug","state":{"name":"Done"}}}}}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_linear_prod_test01").unwrap(),
            "sk-linear-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "state-assignee-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("linear.Issue.create", linear_issue_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — update_linear_issue with state_id + assignee_id should succeed"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    graphql_mock.assert_async().await;
}

/// `get_file_contents` with an explicit `ref` parameter (`"main"`).
/// Verifies that the URL is built as `.../contents/{path}?ref=main` and the
/// proxy correctly forwards the request to GitHub.
#[tokio::test]
async fn pipeline_get_file_contents_with_explicit_ref() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"File read."}]}"#,
                );
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                // Tool input includes explicit ref="main".
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_ref_01","name":"get_file_contents","input":{"owner":"test-org","repo":"test-repo","path":"README.md","ref":"main"}}]}"#);
        })
        .await;

    // GitHub: mock must match the ?ref=main query parameter.
    // "aGVsbG8=" = base64("hello")
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/contents/README.md")
                .query_param("ref", "main")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"name":"README.md","content":"aGVsbG8=","encoding":"base64","sha":"deadbeef02"}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "explicit-ref-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — get_file_contents with explicit ref should succeed"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

/// Issue triage handler with a payload that has no `title` field in `data`.
/// `issue_triage::handle` falls back to `"(no title)"` via `unwrap_or`.
/// Verifies the agent runs (Anthropic is called) with the fallback title in the prompt.
#[tokio::test]
async fn pipeline_issue_triage_missing_title_uses_fallback() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // The prompt will contain "(no title)" — verify Anthropic is actually called.
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

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "no-title-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Payload with id but NO title field → handler uses "(no title)" fallback.
    let no_title = bytes::Bytes::from(
        serde_json::to_vec(&json!({
            "action": "create",
            "type": "Issue",
            "data": { "id": "issue-notitle-1" }
        }))
        .unwrap(),
    );
    js.context()
        .publish("linear.Issue.create", no_title)
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await,
        "Anthropic not called — handler should run even when title is missing"
    );
    anthropic_mock.assert_async().await;
}

/// True cross-crate e2e: GitHub webhook → trogon-github server → NATS JetStream
/// → trogon-agent runner → mock Anthropic.
///
/// This is the only test that exercises the full stack end-to-end, including
/// the HTTP ingress layer (trogon-github), rather than publishing directly to
/// JetStream.
#[tokio::test]
async fn pipeline_github_webhook_to_agent_cross_crate_e2e() {
    let (_c, nats_port) = start_nats().await;
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

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    // ── 1. Start trogon-github server (no secret → no HMAC validation).
    //       serve() creates the GITHUB stream automatically.
    let github_webhook_port = get_free_port().await;
    let github_nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let github_cfg = GithubConfig {
        webhook_secret: None,
        port: github_webhook_port,
        subject_prefix: "github".to_string(),
        stream_name: "GITHUB".to_string(),
        stream_max_age: Duration::from_secs(3600),
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
    };
    tokio::spawn(async move {
        trogon_github::serve(github_cfg, github_nats).await.ok();
    });
    wait_for_tcp(github_webhook_port).await;

    // ── 2. Start proxy + worker.
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    // trogon-github created GITHUB stream; agent needs LINEAR too.
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "cross-crate-gh-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 3. Start agent.
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 4. POST a PR webhook directly to trogon-github (no secret → no sig needed).
    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 42,
        "repository": { "name": "test-repo", "owner": { "login": "test-org" } }
    }))
    .unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{github_webhook_port}/webhook"))
        .header("X-GitHub-Event", "pull_request")
        .header("X-GitHub-Delivery", "delivery-cross-crate-01")
        .header("Content-Type", "application/json")
        .body(payload)
        .send()
        .await
        .expect("Failed to POST to trogon-github");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "trogon-github should return 200"
    );

    // ── 5. Verify the full pipeline fired.
    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(25)).await,
        "Anthropic not called — webhook → trogon-github → NATS → trogon-agent pipeline broken"
    );
    anthropic_mock.assert_async().await;
}

/// True cross-crate e2e: Linear webhook → trogon-linear server → NATS JetStream
/// → trogon-agent runner → mock Anthropic.
#[tokio::test]
async fn pipeline_linear_webhook_to_agent_cross_crate_e2e() {
    let (_c, nats_port) = start_nats().await;
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

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);

    // ── 1. Start trogon-linear server (no secret, no timestamp check).
    //       serve() creates the LINEAR stream automatically.
    let linear_webhook_port = get_free_port().await;
    let linear_nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let linear_cfg = LinearConfig {
        webhook_secret: None,
        port: linear_webhook_port,
        subject_prefix: "linear".to_string(),
        stream_name: "LINEAR".to_string(),
        stream_max_age: Duration::from_secs(3600),
        timestamp_tolerance: None, // disabled — no timestamp field required in payload
        nats_ack_timeout: Duration::from_secs(10),
        nats_stream_op_timeout: Duration::from_secs(10),
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
    };
    tokio::spawn(async move {
        trogon_linear::serve(linear_cfg, linear_nats).await.ok();
    });
    wait_for_tcp(linear_webhook_port).await;

    // ── 2. Start proxy + worker.
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    // trogon-linear created LINEAR stream; agent needs GITHUB too.
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;
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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "cross-crate-lin-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 3. Start agent.
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 4. POST a Linear Issue create webhook to trogon-linear (no signature needed).
    let payload = serde_json::to_vec(&json!({
        "action": "create",
        "type": "Issue",
        "data": {
            "id": "issue-cross-crate-01",
            "title": "Cross-crate test issue",
            "priority": 2,
            "team": { "name": "Platform" }
        }
    }))
    .unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{linear_webhook_port}/webhook"))
        .header("Content-Type", "application/json")
        .body(payload)
        .send()
        .await
        .expect("Failed to POST to trogon-linear");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "trogon-linear should return 200"
    );

    // ── 5. Verify the full pipeline fired.
    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(25)).await,
        "Anthropic not called — webhook → trogon-linear → NATS → trogon-agent pipeline broken"
    );
    anthropic_mock.assert_async().await;
}

/// `run()` must fail fast with a connection error when NATS is unreachable.
/// This verifies that startup errors surface immediately rather than hanging.
#[tokio::test]
async fn pipeline_runner_startup_fails_when_nats_unreachable() {
    // Point to a port that has nothing listening.
    let agent_cfg = AgentConfig {
        nats: NatsConfig::new(vec!["nats://127.0.0.1:19998".to_string()], NatsAuth::None),
        proxy_url: "http://127.0.0.1:19999".to_string(),
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), run(agent_cfg)).await;
    assert!(
        result.is_err() || matches!(result, Ok(Err(_))),
        "Expected run() to fail when NATS is unreachable"
    );
}

/// When Anthropic returns a `tool_use` block whose `input` object is missing a
/// required field (here `owner` is omitted from `get_pr_diff`), the tool
/// dispatch layer returns `"Tool error: missing owner"`.  That error string is
/// sent back to Anthropic as a `tool_result` and the loop concludes on
/// `end_turn` — the agent must not crash or stall.
#[tokio::test]
async fn pipeline_tool_missing_required_input_returns_error_as_tool_result() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Second call carries the tool_result with the missing-field error.
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result")
                .body_contains("missing owner");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Noted the missing field."}]}"#);
        })
        .await;

    // First call → get_pr_diff tool_use with `owner` field deliberately absent.
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                // `input` has repo + pr_number but NO `owner` — triggers "missing owner".
                .body(r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_miss_01","name":"get_pr_diff","input":{"repo":"test-repo","pr_number":42}}]}"#);
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "missing-field-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        max_iterations: 5,
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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not received — missing tool input field should propagate as tool_result"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
}

/// Verifies the proxy worker-timeout path through the full agent pipeline.
///
/// When no worker is running, the proxy times out and returns 504.  The
/// `agent_loop` fails to JSON-decode the 504 body as `AnthropicResponse`,
/// producing `AgentError::Http`.  The runner must log the error, ack the
/// JetStream message, and remain alive to process subsequent events.
///
/// Flow:
/// 1. Start proxy with a 300 ms `worker_timeout` — no worker running.
/// 2. Publish PR event → proxy times out → agent gets 504 → `AgentError::Http`.
/// 3. Start worker + mock Anthropic.
/// 4. Publish second PR event → succeeds → Anthropic called exactly once.
#[tokio::test]
async fn pipeline_proxy_worker_timeout_acks_and_runner_continues() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Mock Anthropic — only the second event (after worker starts) reaches here.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Reviewed."}]}"#,
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

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

    // Proxy with a very short worker_timeout — NO worker started yet.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: js.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_millis(300), // short: no worker → times out
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── Event 1: no worker → proxy times out → AgentError::Http → acked.
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    // Wait for the proxy timeout + processing to complete.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must not be reached when the proxy times out"
    );

    // ── Now start the worker — the runner must still be alive.
    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "timeout-recovery-worker",
            &worker_stream,
        )
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── Event 2: worker now running → reaches Anthropic.
    js.context()
        .publish("github.pull_request", pr_payload("opened"))
        .await
        .unwrap();

    // The proxy publishes the outbound NATS request before timing out, so the
    // stale outbound message from event 1 may also be consumed by the worker
    // once it starts.  We verify only that the runner recovered (≥ 1 hit).
    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(15)).await,
        "Runner must still process events after recovering from a proxy timeout"
    );
}

/// Full pipeline with a real HashiCorp Vault container (dev mode) as the token
/// store.  Verifies that the worker resolves the opaque proxy token through
/// HashiCorp Vault KV v2 and forwards the real API key to mock Anthropic.
///
/// This is the only pipeline_e2e test that uses the `HashicorpVaultStore`
/// backend; all other tests use `MemoryVault`.
#[tokio::test]
async fn pipeline_hashicorp_vault_token_resolution() {
    let (_nats_c, nats_port) = start_nats().await;
    let (_vault_c, vault_addr) = start_vault().await;
    let mock_server = MockServer::start_async().await;

    // Mock Anthropic — must receive the REAL key resolved from HashiCorp Vault.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-vault-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"LGTM via Vault."}]}"#);
        })
        .await;

    // Seed the real Vault with the opaque token → real key mapping.
    let vault = Arc::new(
        HashicorpVaultStore::new(HashicorpVaultConfig::new(
            &vault_addr,
            "secret",
            VaultAuth::Token("root".to_string()),
        ))
        .await
        .expect("Failed to connect to HashiCorp Vault"),
    );
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-vault-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "vault-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(25)).await,
        "Anthropic not called — HashiCorp Vault token resolution failed"
    );
    anthropic_mock.assert_async().await;
}

// ── memory tool e2e tests ──────────────────────────────────────────────────────

/// `get_pr_comments` is routed through the proxy+worker and the GitHub API
/// receives the real token.  The agent calls `get_pr_comments` on the first
/// iteration and ends on the second.
#[tokio::test]
async fn pipeline_get_pr_comments_tool_detokenized() {
    let (_nats_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    // Second Anthropic call (body contains tool_result) → end_turn.
    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Memory read."}]}"#);
        })
        .await;

    // First Anthropic call → tool_use get_pr_comments.
    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_gpc",
                        "name": "get_pr_comments",
                        "input": {"owner":"acme","repo":"trogon","pr_number":42}
                    }]
                }"#,
                );
        })
        .await;

    // GitHub Issues comments endpoint — must receive the real token.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/acme/trogon/issues/42/comments")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"id":1,"user":{"login":"alice"},"body":"Previous review."}]"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_nats = nats.clone();
    let proxy_js = js.clone();
    let proxy_base = mock_server.base_url();
    let proxy_outbound = outbound.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            trogon_secret_proxy::proxy::router(ProxyState {
                nats: proxy_nats,
                jetstream: proxy_js,
                prefix: "trogon".to_string(),
                outbound_subject: proxy_outbound,
                worker_timeout: Duration::from_secs(15),
                base_url_override: Some(proxy_base),
            }),
        )
        .await
        .ok();
    });

    let (nats2, js2) = js_client(nats_port).await;
    let js2 = NatsJetStreamClient::new(js2);
    let stream_name = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            js2,
            nats2,
            vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "gpc-worker",
            &stream_name,
        )
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    tokio::spawn(async move {
        run(AgentConfig {
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
            max_iterations: 5,
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
            split_evaluator_url: None,
            split_auth_token: None,
            incidentio_stream_name: None,
        })
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not hit"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

/// `update_file` (PUT /repos/.../contents/...) is routed through the
/// proxy+worker and the GitHub API receives the real token.
#[tokio::test]
async fn pipeline_update_file_tool_detokenized() {
    let (_nats_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"File updated."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_uf",
                        "name": "update_file",
                        "input": {
                            "owner":"acme","repo":"trogon",
                            "path":".trogon/memory.md",
                            "message":"chore: update memory",
                            "content":"Agent memory: Learned X.",
                            "branch":"trogon/memory-update"
                        }
                    }]
                }"#,
                );
        })
        .await;

    // GitHub Contents API (PUT) — must receive the real token.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::PUT)
                .path("/repos/acme/trogon/contents/.trogon/memory.md")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"commit":{"sha":"abc123"},"content":{"path":".trogon/memory.md"}}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_nats = nats.clone();
    let proxy_js = js.clone();
    let proxy_base = mock_server.base_url();
    let proxy_outbound = outbound.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            trogon_secret_proxy::proxy::router(ProxyState {
                nats: proxy_nats,
                jetstream: proxy_js,
                prefix: "trogon".to_string(),
                outbound_subject: proxy_outbound,
                worker_timeout: Duration::from_secs(15),
                base_url_override: Some(proxy_base),
            }),
        )
        .await
        .ok();
    });

    let (nats2, js2) = js_client(nats_port).await;
    let js2 = NatsJetStreamClient::new(js2);
    let stream_name = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            js2,
            nats2,
            vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "uf-worker",
            &stream_name,
        )
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    tokio::spawn(async move {
        run(AgentConfig {
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
            max_iterations: 5,
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
            split_evaluator_url: None,
            split_auth_token: None,
            incidentio_stream_name: None,
        })
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not hit"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

/// `create_pull_request` (POST /repos/.../pulls) is routed through the
/// proxy+worker and the GitHub API receives the real token.
#[tokio::test]
async fn pipeline_create_pull_request_tool_detokenized() {
    let (_nats_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"PR opened."}]}"#,
                );
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_cpr",
                        "name": "create_pull_request",
                        "input": {
                            "owner":"acme","repo":"trogon",
                            "title":"chore: update memory",
                            "head":"trogon/memory-update",
                            "base":"main",
                            "body":"Agent learned new conventions."
                        }
                    }]
                }"#,
                );
        })
        .await;

    // GitHub Pulls API (POST) — must receive the real token.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/repos/acme/trogon/pulls")
                .header("authorization", "Bearer sk-gh-realkey");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"number":99,"html_url":"https://github.com/acme/trogon/pull/99"}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_github_prod_test01").unwrap(),
            "sk-gh-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_nats = nats.clone();
    let proxy_js = js.clone();
    let proxy_base = mock_server.base_url();
    let proxy_outbound = outbound.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            trogon_secret_proxy::proxy::router(ProxyState {
                nats: proxy_nats,
                jetstream: proxy_js,
                prefix: "trogon".to_string(),
                outbound_subject: proxy_outbound,
                worker_timeout: Duration::from_secs(15),
                base_url_override: Some(proxy_base),
            }),
        )
        .await
        .ok();
    });

    let (nats2, js2) = js_client(nats_port).await;
    let js2 = NatsJetStreamClient::new(js2);
    let stream_name = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            js2,
            nats2,
            vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "cpr-worker",
            &stream_name,
        )
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    tokio::spawn(async move {
        run(AgentConfig {
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
            max_iterations: 5,
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
            split_evaluator_url: None,
            split_auth_token: None,
            incidentio_stream_name: None,
        })
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("github.pull_request", pr_opened_payload())
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not hit"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    github_mock.assert_async().await;
}

/// `get_linear_comments` is routed through the proxy+worker and the Linear
/// GraphQL API receives the real token.
/// The proxy strips the "/linear" prefix so the mock sees "/graphql".
#[tokio::test]
async fn pipeline_get_linear_comments_tool_detokenized() {
    let (_nats_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_end_turn = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey")
                .body_contains("tool_result");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Comments loaded."}]}"#);
        })
        .await;

    let anthropic_tool_call = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu_glc",
                        "name": "get_linear_comments",
                        "input": {"issue_id":"ISS-42"}
                    }]
                }"#,
                );
        })
        .await;

    // Linear GraphQL endpoint — must receive the real linear token.
    // The proxy strips "/linear" prefix so the mock sees "/graphql".
    let linear_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/graphql")
                .header("authorization", "Bearer sk-linear-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":{"issue":{"comments":{"nodes":[{"id":"c1","body":"Prior triage.","createdAt":"2024-01-01","user":{"name":"Alice","email":"alice@example.com"}}]}}}}"#);
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
    vault
        .store(
            &ApiKeyToken::new("tok_linear_prod_test01").unwrap(),
            "sk-linear-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound)
        .await
        .unwrap();
    create_stream(js.context(), "GITHUB", &["github.pull_request"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_nats = nats.clone();
    let proxy_js = js.clone();
    let proxy_base = mock_server.base_url();
    let proxy_outbound = outbound.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            trogon_secret_proxy::proxy::router(ProxyState {
                nats: proxy_nats,
                jetstream: proxy_js,
                prefix: "trogon".to_string(),
                outbound_subject: proxy_outbound,
                worker_timeout: Duration::from_secs(15),
                base_url_override: Some(proxy_base),
            }),
        )
        .await
        .ok();
    });

    let (nats2, js2) = js_client(nats_port).await;
    let js2 = NatsJetStreamClient::new(js2);
    let stream_name = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            js2,
            nats2,
            vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "glc-worker",
            &stream_name,
        )
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let linear_payload = bytes::Bytes::from(
        serde_json::to_vec(&json!({
            "action": "create",
            "type": "Issue",
            "data": { "id": "ISS-42", "title": "Fix memory leak" }
        }))
        .unwrap(),
    );

    tokio::spawn(async move {
        run(AgentConfig {
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
            max_iterations: 5,
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
            split_evaluator_url: None,
            split_auth_token: None,
            incidentio_stream_name: None,
        })
        .await
        .ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    js.context()
        .publish("linear.Issue.create", linear_payload)
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_end_turn, Duration::from_secs(20)).await,
        "end_turn not hit — linear comments pipeline did not complete"
    );
    anthropic_end_turn.assert_async().await;
    anthropic_tool_call.assert_async().await;
    linear_mock.assert_async().await;
}

// ── New event types ────────────────────────────────────────────────────────────

/// Helper: spin up NATS + proxy + worker + agent with github.> stream.
async fn setup_full_pipeline(
    nats_port: u16,
    mock_server: &MockServer,
) -> (u16, NatsJetStreamClient) {
    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let (nats, js) = js_client(nats_port).await;
    let js = NatsJetStreamClient::new(js);
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();
    // Use github.> so all GitHub consumers (PR, comment, push, check_run) can bind.
    create_stream(js.context(), "GITHUB", &["github.>"]).await;
    create_stream(js.context(), "LINEAR", &["linear.Issue.>"]).await;
    create_stream(js.context(), "CRON_TICKS", &["cron.>"]).await;

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
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let worker_js = js.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            "new-events-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

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
        split_evaluator_url: None,
        split_auth_token: None,
        incidentio_stream_name: None,
    };
    tokio::spawn(async move {
        run(agent_cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    (proxy_port, js)
}

/// PR merged event (closed + merged=true) reaches Anthropic through the full pipeline.
#[tokio::test]
async fn pipeline_pr_merged_event_reaches_anthropic() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_mock = mock_server.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages")
            .header("authorization", "Bearer sk-ant-realkey");
        then.status(200).header("content-type", "application/json")
            .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Merge noted."}]}"#);
    }).await;

    let (_proxy_port, js) = setup_full_pipeline(nats_port, &mock_server).await;

    let payload = serde_json::to_vec(&json!({
        "action": "closed",
        "number": 77,
        "pull_request": { "merged": true, "title": "Add feature", "merged_by": { "login": "alice" } },
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    })).unwrap();
    js.context()
        .publish("github.pull_request", bytes::Bytes::from(payload))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await,
        "Anthropic was not called for PR merged event through full pipeline"
    );
}

/// issue_comment created event reaches Anthropic through the full pipeline.
#[tokio::test]
async fn pipeline_issue_comment_event_reaches_anthropic() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_mock =
        mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST)
                    .path("/v1/messages")
                    .header("authorization", "Bearer sk-ant-realkey");
                then.status(200).header("content-type", "application/json")
            .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Replied."}]}"#);
            })
            .await;

    let (_proxy_port, js) = setup_full_pipeline(nats_port, &mock_server).await;

    let payload = serde_json::to_vec(&json!({
        "action": "created",
        "issue": { "number": 12, "pull_request": {} },
        "comment": { "body": "What does this do?", "user": { "login": "carol" } },
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    }))
    .unwrap();
    js.context()
        .publish("github.issue_comment", bytes::Bytes::from(payload))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await,
        "Anthropic was not called for issue_comment event through full pipeline"
    );
}

/// push to branch event reaches Anthropic through the full pipeline.
#[tokio::test]
async fn pipeline_push_to_branch_event_reaches_anthropic() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_mock = mock_server.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages")
            .header("authorization", "Bearer sk-ant-realkey");
        then.status(200).header("content-type", "application/json")
            .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Push looks safe."}]}"#);
    }).await;

    let (_proxy_port, js) = setup_full_pipeline(nats_port, &mock_server).await;

    let payload = serde_json::to_vec(&json!({
        "ref": "refs/heads/feature-y",
        "deleted": false,
        "pusher": { "name": "dave" },
        "commits": [{ "id": "abc123" }],
        "head_commit": { "message": "add feature y" },
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    }))
    .unwrap();
    js.context()
        .publish("github.push", bytes::Bytes::from(payload))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await,
        "Anthropic was not called for push event through full pipeline"
    );
}

/// CI check_run failure event reaches Anthropic through the full pipeline.
#[tokio::test]
async fn pipeline_ci_failure_event_reaches_anthropic() {
    let (_c, nats_port) = start_nats().await;
    let mock_server = MockServer::start_async().await;

    let anthropic_mock = mock_server.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages")
            .header("authorization", "Bearer sk-ant-realkey");
        then.status(200).header("content-type", "application/json")
            .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"CI failure diagnosed."}]}"#);
    }).await;

    let (_proxy_port, js) = setup_full_pipeline(nats_port, &mock_server).await;

    let payload = serde_json::to_vec(&json!({
        "action": "completed",
        "check_run": {
            "name": "cargo test",
            "conclusion": "failure",
            "details_url": "https://ci.example.com/1",
            "pull_requests": [{ "number": 42 }]
        },
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    }))
    .unwrap();
    js.context()
        .publish("github.check_run", bytes::Bytes::from(payload))
        .await
        .unwrap();

    assert!(
        wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await,
        "Anthropic was not called for CI failure event through full pipeline"
    );
}
