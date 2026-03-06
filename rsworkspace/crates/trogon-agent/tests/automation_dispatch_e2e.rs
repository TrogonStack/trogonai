//! End-to-end tests for the automation dispatch path.
//!
//! Verifies:
//! 1. A matching automation's prompt is forwarded to the model (not the hardcoded handler).
//! 2. When an automation is present, the hardcoded fallback handler is NOT called.
//! 3. When no automation matches, the hardcoded fallback handler IS called.
//! 4. A disabled automation is skipped → fallback handler runs.
//! 5. The automations HTTP API responds on `api_port` while the runner is live.
//! 6. Automation-level MCP server tools are included in the Anthropic request.
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test automation_dispatch_e2e -- --test-threads=1

use std::time::Duration;

use async_nats::jetstream;
use httpmock::MockServer;
use serde_json::json;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent::{AgentConfig, run};
use trogon_automations::{Automation, AutomationStore, McpServer};
use trogon_nats::{NatsAuth, NatsConfig};

// ── helpers ───────────────────────────────────────────────────────────────────

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
        .expect("connect NATS");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

async fn create_streams(js: &jetstream::Context) {
    for (name, subject) in [("GITHUB", "github.>"), ("LINEAR", "linear.Issue.>")] {
        js.get_or_create_stream(jetstream::stream::Config {
            name: name.to_string(),
            subjects: vec![subject.to_string()],
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("create stream {name}: {e}"));
    }
}

fn runner_cfg(nats_port: u16, proxy_url: String, tenant_id: &str, api_port: u16) -> AgentConfig {
    AgentConfig {
        nats: NatsConfig::new(vec![format!("nats://127.0.0.1:{nats_port}")], NatsAuth::None),
        proxy_url,
        anthropic_token: "test-token".to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: String::new(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 1,
        github_stream_name: None,
        linear_stream_name: None,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port,
        tenant_id: tenant_id.to_string(),
    }
}

fn make_automation(id: &str, tenant_id: &str, trigger: &str, prompt: &str) -> Automation {
    Automation {
        id: id.to_string(),
        tenant_id: tenant_id.to_string(),
        name: format!("Test automation {id}"),
        trigger: trigger.to_string(),
        prompt: prompt.to_string(),
        tools: vec![],
        memory_path: None,
        mcp_servers: vec![],
        enabled: true,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn pr_opened_payload() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "action": "opened",
        "number": 42,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "Add feature" }
    }))
    .unwrap()
}

fn end_turn_body() -> &'static str {
    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}"#
}

/// Wait until a mock has been hit at least `min_hits` times, or the timeout expires.
async fn wait_for_hits(mock: &httpmock::Mock<'_>, min_hits: usize, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if mock.hits_async().await >= min_hits {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A PR event that matches a stored automation uses the automation's custom prompt.
#[tokio::test]
async fn automation_dispatch_uses_automation_prompt() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation("auto-1", "default", "github.pull_request", "CUSTOM_AUTOMATION_PROMPT_XYZ"))
        .await
        .expect("put automation");

    let anthropic = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("CUSTOM_AUTOMATION_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default", 0)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into()).await.expect("publish");

    assert!(
        wait_for_hits(&anthropic, 1, Duration::from_secs(8)).await,
        "automation prompt was not forwarded to the model"
    );
}

/// When an automation matches, the hardcoded fallback handler is NOT called.
#[tokio::test]
async fn automation_dispatch_overrides_fallback_handler() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation("auto-2", "default", "github.pull_request", "MY_AUTOMATION_PROMPT"))
        .await
        .expect("put automation");

    // Fires only when the fallback handler's "code reviewer" prompt appears — should be 0 hits.
    let fallback_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("code reviewer");
            then.status(500).body("fallback should not be called");
        })
        .await;

    // Catch-all for the automation path.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default", 0)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into()).await.expect("publish");
    tokio::time::sleep(Duration::from_secs(5)).await;

    assert_eq!(
        fallback_mock.hits_async().await, 0,
        "fallback handler was called despite matching automation"
    );
}

/// When no automation matches the event, the hardcoded fallback handler runs.
#[tokio::test]
async fn no_automation_falls_back_to_hardcoded_handler() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;
    // Store is empty — no automation registered.

    // pr_review fallback sends "code reviewer" in its prompt.
    let fallback_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("code reviewer");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default", 0)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into()).await.expect("publish");

    assert!(
        wait_for_hits(&fallback_mock, 1, Duration::from_secs(8)).await,
        "fallback handler (pr_review) was not called when store is empty"
    );
}

/// A disabled automation is not dispatched; the fallback handler runs instead.
#[tokio::test]
async fn disabled_automation_falls_back_to_hardcoded_handler() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    let mut auto = make_automation("auto-disabled", "default", "github.pull_request", "DISABLED_PROMPT");
    auto.enabled = false;
    store.put(&auto).await.expect("put disabled automation");

    // If the disabled automation somehow fires → 500 (test fails via result check below).
    let bad_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("DISABLED_PROMPT");
            then.status(500).body("disabled automation must not fire");
        })
        .await;

    // Fallback handler mock.
    let fallback_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("code reviewer");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default", 0)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into()).await.expect("publish");

    assert!(
        wait_for_hits(&fallback_mock, 1, Duration::from_secs(8)).await,
        "fallback handler was not called for disabled automation"
    );
    assert_eq!(bad_mock.hits_async().await, 0, "disabled automation must not fire");
}

/// The automations HTTP API is reachable on `api_port` while the runner is live.
#[tokio::test]
async fn api_server_accessible_during_runner_run() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    // Find a free port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port)).await.ok()
    });

    // Poll until the API server accepts connections (up to 10s).
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{api_port}/automations");
    let resp = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match client.get(&url).header("x-tenant-id", "acme").send().await {
                Ok(r) => break r,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => panic!("API server never came up: {e}"),
            }
        }
    };

    assert_eq!(resp.status(), 200, "automations API must return 200");
    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    assert_eq!(body, json!([]), "empty store returns empty array");
}

/// Automation-level MCP server tools appear in the Anthropic request body.
#[tokio::test]
async fn automation_mcp_server_tools_forwarded_to_model() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    // MCP initialize handshake.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST).body_contains("\"initialize\"");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0", "id": 1,
                "result": { "protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {} }
            }));
    })
    .await;

    // MCP tools/list response — advertises one tool.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST).body_contains("tools/list");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "jsonrpc": "2.0", "id": 2,
                "result": {
                    "tools": [{
                        "name": "search_docs",
                        "description": "Search documentation",
                        "inputSchema": { "type": "object", "properties": {} }
                    }]
                }
            }));
    })
    .await;

    // Anthropic mock: verify the prefixed MCP tool name is in the tools list.
    let anthropic_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("mcp__search_mcp__search_docs");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    let store = AutomationStore::open(&js).await.expect("open store");
    let mut auto = make_automation("auto-mcp", "default", "github.pull_request", "MCP_TEST_PROMPT");
    auto.mcp_servers = vec![McpServer {
        name: "search_mcp".to_string(),
        url: format!("{}/mcp", mock_url),
    }];
    store.put(&auto).await.expect("put automation");

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default", 0)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into()).await.expect("publish");

    assert!(
        wait_for_hits(&anthropic_mock, 1, Duration::from_secs(8)).await,
        "MCP tool 'mcp__search_mcp__search_docs' was not included in the Anthropic request"
    );
}
