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
}

fn runner_cfg_with_cron(
    nats_port: u16,
    proxy_url: String,
    tenant_id: &str,
    api_port: u16,
    cron_stream: Option<String>,
) -> AgentConfig {
    AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url,
        anthropic_token: "test-token".to_string(),
        github_token: "tok_github_prod_test01".to_string(),
        linear_token: "tok_linear_prod_test01".to_string(),
        slack_token: String::new(),
        model: "claude-opus-4-6".to_string(),
        max_iterations: 1,
        github_stream_name: None,
        linear_stream_name: None,
        cron_stream_name: cron_stream,
        datadog_stream_name: None,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port,
        tenant_id: tenant_id.to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    }
}

fn runner_cfg(nats_port: u16, proxy_url: String, tenant_id: &str, api_port: u16) -> AgentConfig {
    AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url,
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
        tenant_id: tenant_id.to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
    }
}

fn make_automation(id: &str, tenant_id: &str, trigger: &str, prompt: &str) -> Automation {
    Automation {
        id: id.to_string(),
        tenant_id: tenant_id.to_string(),
        name: format!("Test automation {id}"),
        trigger: trigger.to_string(),
        prompt: prompt.to_string(),
        model: None,
        tools: vec![],
        memory_path: None,
        mcp_servers: vec![],
        enabled: true,
        visibility: trogon_automations::Visibility::Private,
        variables: std::collections::HashMap::new(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
        skill_ids: vec![],
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
        .put(&make_automation(
            "auto-1",
            "default",
            "github.pull_request",
            "CUSTOM_AUTOMATION_PROMPT_XYZ",
        ))
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

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

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
        .put(&make_automation(
            "auto-2",
            "default",
            "github.pull_request",
            "MY_AUTOMATION_PROMPT",
        ))
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
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");
    tokio::time::sleep(Duration::from_secs(5)).await;

    assert_eq!(
        fallback_mock.hits_async().await,
        0,
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

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

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
    let mut auto = make_automation(
        "auto-disabled",
        "default",
        "github.pull_request",
        "DISABLED_PROMPT",
    );
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

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&fallback_mock, 1, Duration::from_secs(8)).await,
        "fallback handler was not called for disabled automation"
    );
    assert_eq!(
        bad_mock.hits_async().await,
        0,
        "disabled automation must not fire"
    );
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
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
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
        when.method(httpmock::Method::POST)
            .body_contains("\"initialize\"");
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
        when.method(httpmock::Method::POST)
            .body_contains("tools/list");
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
    let mut auto = make_automation(
        "auto-mcp",
        "default",
        "github.pull_request",
        "MCP_TEST_PROMPT",
    );
    auto.mcp_servers = vec![McpServer {
        name: "search_mcp".to_string(),
        url: format!("{}/mcp", mock_url),
    }];
    store.put(&auto).await.expect("put automation");

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&anthropic_mock, 1, Duration::from_secs(8)).await,
        "MCP tool 'mcp__search_mcp__search_docs' was not included in the Anthropic request"
    );
}

/// Automation with a `model` override sends that model to the Anthropic proxy.
#[tokio::test]
async fn automation_model_override_sent_to_anthropic() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    let mut auto = make_automation(
        "auto-model",
        "default",
        "github.pull_request",
        "MODEL_OVERRIDE_PROMPT",
    );
    auto.model = Some("claude-haiku-4-5-20251001".to_string());
    store.put(&auto).await.expect("put automation");

    // Match requests that contain the haiku model ID in the body.
    let haiku_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("claude-haiku-4-5-20251001");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&haiku_mock, 1, Duration::from_secs(8)).await,
        "automation model override was not forwarded to Anthropic"
    );
}

/// A cron tick published to CRON_TICKS dispatches to a matching automation.
#[tokio::test]
async fn cron_tick_dispatches_to_matching_automation() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (nats, js) = js_client(nats_port).await;
    create_streams(&js).await; // creates GITHUB, LINEAR, and CRON_TICKS

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-cron",
            "default",
            "cron.daily",
            "CRON_DAILY_PROMPT_XYZ",
        ))
        .await
        .expect("put cron automation");

    let anthropic = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("CRON_DAILY_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move {
        run(runner_cfg_with_cron(
            nats_port,
            mock_url,
            "default",
            0,
            Some("CRON_TICKS".to_string()),
        ))
        .await
        .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Publish a cron tick to the matching subject.
    nats.publish("cron.daily", b"{}".as_slice().into())
        .await
        .expect("publish cron tick");

    assert!(
        wait_for_hits(&anthropic, 1, Duration::from_secs(8)).await,
        "cron tick did not dispatch to matching automation"
    );
}

/// After dispatch, a RunRecord is persisted in RunStore and accessible via HTTP.
#[tokio::test]
async fn dispatch_persists_run_record_accessible_via_api() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-runs",
            "default",
            "github.pull_request",
            "RUNS_RECORD_PROMPT",
        ))
        .await
        .expect("put automation");

    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    // Find a free port for the API.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    // Wait for API to come up.
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    // Trigger an automation.
    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Poll until at least one RunRecord appears (up to 10s).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let runs = loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 1, "exactly one run should be recorded");
    assert_eq!(arr[0]["automation_id"], "auto-runs");
    assert_eq!(arr[0]["status"], "success");
    assert!(!arr[0]["id"].as_str().unwrap_or("").is_empty());

    // Stats should also reflect the run.
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

    // Verify the RunRecord fields are correct and consistent.
    let run = &arr[0];
    assert_eq!(run["automation_id"], "auto-runs");
    assert_eq!(run["tenant_id"], "default");
    assert_eq!(run["nats_subject"], "github.pull_request");
    assert!(run["started_at"].as_u64().unwrap() > 0);
    assert!(run["finished_at"].as_u64().unwrap() >= run["started_at"].as_u64().unwrap());
}

/// When the Anthropic proxy returns 500, the dispatch records a `failed` RunRecord.
#[tokio::test]
async fn dispatch_records_failed_run_on_agent_error() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-fail",
            "default",
            "github.pull_request",
            "FAIL_PROMPT",
        ))
        .await
        .expect("put automation");

    // Anthropic proxy always returns 500 → agent error → RunStatus::Failed.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(500).body("Internal Server Error");
    })
    .await;

    // Find a free port for the API.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    // Wait for API to come up.
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    // Trigger an automation.
    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Poll until a RunRecord with status=failed appears.
    // The agent retries 500s up to 3 times with 2s+4s+8s backoff (14s total)
    // before giving up and recording the failure — allow enough headroom.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    let runs = loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["automation_id"], "auto-fail");
    assert_eq!(arr[0]["status"], "failed");

    // Stats must reflect the failure.
    let stats: serde_json::Value = client
        .get(format!("http://127.0.0.1:{api_port}/stats"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats["failed_7d"], 1);
    assert_eq!(stats["successful_7d"], 0);
}

/// After an automation run completes, the `AgentPromise` in NATS KV must be
/// `Resolved`. This verifies the full promise lifecycle: Running → Resolved.
#[tokio::test]
async fn dispatch_marks_promise_resolved_in_kv() {
    use trogon_agent::promise_store::{PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-kv",
            "default",
            "github.pull_request",
            "KV_PROMISE_PROMPT",
        ))
        .await
        .expect("put automation");

    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    // Find a free port for the API.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    // Wait for the API to come up.
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    // Trigger an automation.
    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Poll until a RunRecord appears — the promise is marked Resolved before the
    // RunRecord is stored, so once we see the RunRecord the KV entry is ready.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Open a PromiseStore on the same NATS connection and verify the promise.
    // Promise ID = "{subject_slug}.{seq}.{auto_id}" = "github_pull_request.1.auto-kv"
    // (first message on the GITHUB stream → seq = 1).
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let (promise, _rev) = ps
        .get_promise("default", "github_pull_request.1.auto-kv")
        .await
        .expect("get_promise")
        .expect("promise must exist in KV after run completes");

    assert_eq!(
        promise.status,
        PromiseStatus::Resolved,
        "promise must be Resolved after a successful run"
    );
    assert_eq!(promise.automation_id, "auto-kv");
    assert_eq!(promise.tenant_id, "default");
}

/// Startup recovery: a stale `Running` promise (simulating a crashed process) is
/// detected at startup, the automation is re-run from its checkpoint, and the
/// promise is marked `Resolved` in KV.
#[tokio::test]
async fn startup_recovery_resumes_stale_automation_promise_to_resolved() {
    use trogon_agent::promise_store::{AgentPromise, PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    // Create the automation so the recovery can find and run it.
    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&make_automation(
            "auto-recover",
            "default",
            "github.pull_request",
            "RECOVERY_PROMPT",
        ))
        .await
        .expect("put automation");

    // Pre-seed a stale Running promise — simulates a process that crashed
    // mid-run 20 minutes ago (well past the 10-minute staleness threshold).
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let stale_claimed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(20 * 60);
    let stale = AgentPromise {
        id: "github_pull_request.99.auto-recover".to_string(),
        tenant_id: "default".to_string(),
        automation_id: "auto-recover".to_string(),
        status: PromiseStatus::Running,
        messages: vec![],
        iteration: 0,
        worker_id: "crashed-worker-pid12345".to_string(),
        claimed_at: stale_claimed_at,
        trigger: serde_json::from_slice(&pr_opened_payload()).unwrap_or_default(),
        nats_subject: "github.pull_request".to_string(),
        system_prompt: None,
        recovery_count: 0,
        checkpoint_degraded: false,
        failure_reason: None,
    };
    ps.put_promise(&stale).await.expect("seed stale promise");

    // Anthropic mock — called once when the recovery re-runs the automation.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    // Find a free port for the API.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    // Wait for the API to come up.
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    // Poll until the recovery produces a RunRecord (proof the run completed).
    // No NATS event is published — the runner finds the stale promise on startup.
    // Allow up to 45 s: startup recovery jitter is 0–30 s plus actual run time.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("Startup recovery did not produce a RunRecord within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The promise must now be Resolved in KV.
    let (promise, _rev) = ps
        .get_promise("default", "github_pull_request.99.auto-recover")
        .await
        .expect("get_promise")
        .expect("promise must still exist in KV");

    assert_eq!(
        promise.status,
        PromiseStatus::Resolved,
        "startup recovery must mark the stale promise Resolved"
    );
    assert_eq!(promise.automation_id, "auto-recover");
}

// ── Startup recovery: PermanentFailed scenarios ───────────────────────────────

/// Helper: poll PromiseStore until the promise reaches PermanentFailed, then return it.
async fn poll_promise_until_permanent_failed(
    ps: &trogon_agent::promise_store::PromiseStore,
    tenant_id: &str,
    promise_id: &str,
    timeout: Duration,
) -> trogon_agent::promise_store::AgentPromise {
    use trogon_agent::promise_store::PromiseStatus;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(Some((p, _))) = ps.get_promise(tenant_id, promise_id).await {
            if p.status == PromiseStatus::PermanentFailed {
                return p;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("Promise {promise_id} did not reach PermanentFailed within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn stale_claimed_at() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(21 * 60) // 21 min ago — past STALE_AFTER_SECS (20 min)
}

/// Startup recovery: a stale promise whose automation is disabled must be marked
/// `PermanentFailed` with reason "automation is disabled" — the automation must
/// NOT be re-run.
#[tokio::test]
async fn startup_recovery_disabled_automation_marks_promise_permanent_failed() {
    use trogon_agent::promise_store::{AgentPromise, PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    // Seed the automation as disabled.
    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    let mut disabled_auto = make_automation(
        "auto-disabled",
        "default",
        "github.pull_request",
        "DISABLED_PROMPT",
    );
    disabled_auto.enabled = false;
    astore.put(&disabled_auto).await.expect("put disabled automation");

    // Seed a stale Running promise pointing at the disabled automation.
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let promise_id = "github_pull_request.77.auto-disabled";
    ps.put_promise(&AgentPromise {
        id: promise_id.to_string(),
        tenant_id: "default".to_string(),
        automation_id: "auto-disabled".to_string(),
        status: PromiseStatus::Running,
        messages: vec![],
        iteration: 0,
        worker_id: "crashed-worker".to_string(),
        claimed_at: stale_claimed_at(),
        trigger: serde_json::from_slice(&pr_opened_payload()).unwrap_or_default(),
        nats_subject: "github.pull_request".to_string(),
        system_prompt: None,
        recovery_count: 0,
        checkpoint_degraded: false,
        failure_reason: None,
    })
    .await
    .expect("seed stale promise");

    // The model must NOT be called — disabled automation is never run.
    let model_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(500).body("must not be called");
        })
        .await;

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });

    // Allow up to 45 s: jitter is 0–30 s plus processing time.
    let promise =
        poll_promise_until_permanent_failed(&ps, "default", promise_id, Duration::from_secs(45))
            .await;

    assert_eq!(
        promise.failure_reason.as_deref(),
        Some("automation is disabled"),
        "failure_reason must explain why the promise was not run"
    );
    assert_eq!(model_mock.hits_async().await, 0, "model must never be called for disabled automation");
}

/// Startup recovery: a stale promise whose automation was deleted (not found in
/// the store) must be marked `PermanentFailed` with reason
/// "automation no longer exists" — without attempting to run anything.
#[tokio::test]
async fn startup_recovery_deleted_automation_marks_promise_permanent_failed() {
    use trogon_agent::promise_store::{AgentPromise, PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    // AutomationStore is open but has NO automation with this ID — simulates deletion.
    let _astore = AutomationStore::open(&js).await.expect("open AutomationStore");

    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let promise_id = "github_pull_request.88.auto-deleted";
    ps.put_promise(&AgentPromise {
        id: promise_id.to_string(),
        tenant_id: "default".to_string(),
        automation_id: "auto-deleted".to_string(), // no matching automation in store
        status: PromiseStatus::Running,
        messages: vec![],
        iteration: 0,
        worker_id: "crashed-worker".to_string(),
        claimed_at: stale_claimed_at(),
        trigger: serde_json::from_slice(&pr_opened_payload()).unwrap_or_default(),
        nats_subject: "github.pull_request".to_string(),
        system_prompt: None,
        recovery_count: 0,
        checkpoint_degraded: false,
        failure_reason: None,
    })
    .await
    .expect("seed stale promise");

    let model_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(500).body("must not be called");
        })
        .await;

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });

    let promise =
        poll_promise_until_permanent_failed(&ps, "default", promise_id, Duration::from_secs(45))
            .await;

    assert_eq!(
        promise.failure_reason.as_deref(),
        Some("automation no longer exists"),
        "failure_reason must explain that the automation was deleted"
    );
    assert_eq!(model_mock.hits_async().await, 0, "model must never be called for deleted automation");
}

/// Startup recovery: a stale built-in-handler promise with an unknown NATS subject
/// (automation_id is empty, subject matches no known handler) must be marked
/// `PermanentFailed` — preventing it from cycling every restart until the 24-hour TTL.
#[tokio::test]
async fn startup_recovery_unknown_subject_marks_promise_permanent_failed() {
    use trogon_agent::promise_store::{AgentPromise, PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    // Create a stream that matches the unknown subject so the runner can
    // list promises without a stream-not-found error.
    js.get_or_create_stream(async_nats::jetstream::stream::Config {
        name: "UNKNOWN_SUBJ".to_string(),
        subjects: vec!["unknown.>".to_string()],
        ..Default::default()
    })
    .await
    .expect("create stream");
    create_streams(&js).await;

    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let promise_id = "unknown_xyz.55.builtin";
    ps.put_promise(&AgentPromise {
        id: promise_id.to_string(),
        tenant_id: "default".to_string(),
        automation_id: "".to_string(), // empty = built-in handler path
        status: PromiseStatus::Running,
        messages: vec![],
        iteration: 0,
        worker_id: "crashed-worker".to_string(),
        claimed_at: stale_claimed_at(),
        trigger: serde_json::Value::Object(serde_json::Map::new()),
        nats_subject: "unknown.xyz.event".to_string(), // no handler for this
        system_prompt: None,
        recovery_count: 0,
        checkpoint_degraded: false,
        failure_reason: None,
    })
    .await
    .expect("seed stale promise");

    let model_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(500).body("must not be called");
        })
        .await;

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });

    let promise =
        poll_promise_until_permanent_failed(&ps, "default", promise_id, Duration::from_secs(45))
            .await;

    assert_eq!(
        promise.status,
        PromiseStatus::PermanentFailed,
        "unknown subject must be marked PermanentFailed to stop cycling"
    );
    assert_eq!(model_mock.hits_async().await, 0, "model must never be called for unknown subject");
}

/// A non-retryable 4xx error from Anthropic (400 Bad Request) marks the promise
/// `PermanentFailed` in KV — retrying this run can never succeed.
#[tokio::test]
async fn dispatch_marks_promise_permanent_failed_on_4xx_error() {
    use trogon_agent::promise_store::{PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&make_automation(
            "auto-pf",
            "default",
            "github.pull_request",
            "PF_PROMPT",
        ))
        .await
        .expect("put automation");

    // 400 Bad Request — non-retryable 4xx → agent marks promise PermanentFailed
    // immediately, without the 2s/4s/8s backoff applied to 5xx errors.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(400).body("bad request");
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Poll until a RunRecord appears with status=failed (promise is written
    // before the RunRecord, so querying KV after this is safe).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let (promise, _rev) = ps
        .get_promise("default", "github_pull_request.1.auto-pf")
        .await
        .expect("get_promise")
        .expect("promise must exist in KV");

    assert_eq!(
        promise.status,
        PromiseStatus::PermanentFailed,
        "non-retryable 4xx must mark the promise PermanentFailed"
    );
}

/// The built-in GitHub PR handler (no matching automation) creates and resolves
/// its promise in KV under key `{tenant_id}.github_pull_request.{seq}` (no automation_id suffix).
#[tokio::test]
async fn builtin_handler_marks_promise_resolved_in_kv() {
    use trogon_agent::promise_store::{PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    // No automations — the runner falls back to the built-in PR handler.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Poll the promise KV directly — built-in handlers don't create RunRecords.
    // Promise ID = "github_pull_request.1" (first message on GITHUB stream, no auto_id suffix).
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let promise = loop {
        if let Ok(Some((p, _))) = ps.get_promise("default", "github_pull_request.1").await {
            if p.status == PromiseStatus::Resolved {
                break p;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("Built-in handler promise never became Resolved within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    assert_eq!(
        promise.automation_id, "",
        "built-in handler promise must have empty automation_id"
    );
    assert_eq!(promise.tenant_id, "default");
    assert_eq!(promise.nats_subject, "github.pull_request");
}

/// A tool-use turn causes a KV checkpoint before the follow-up LLM call.
/// After the run completes, the promise in KV must have `iteration >= 1`
/// (at least one tool exchange was checkpointed) and non-empty `messages`.
#[tokio::test]
async fn multi_turn_run_checkpoints_iteration_and_messages_in_kv() {
    use trogon_agent::promise_store::{PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&make_automation(
            "auto-multiturn",
            "default",
            "github.pull_request",
            "MULTITURN_PROMPT",
        ))
        .await
        .expect("put automation");

    // Second Anthropic call (body contains "tool_result") → end_turn.
    // Registered FIRST so httpmock matches it first (FIFO); the body_contains
    // constraint makes it selective — only fires after a tool result is present.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    // First Anthropic call → tool_use for an unknown tool (no external API call
    // needed; the dispatcher returns an error result, which the agent sends back
    // as a tool_result block on the second call).
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "stop_reason":"tool_use",
                    "content":[{
                        "type":"tool_use",
                        "id":"tu_ckpt_01",
                        "name":"nonexistent_tool_for_checkpoint_test",
                        "input":{}
                    }]
                }"#,
            );
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        // max_iterations must be >= 2 so the second LLM call (after tool result)
        // can reach end_turn; the default runner_cfg uses 1 which would terminate
        // with PermanentFailed (MaxIterationsReached) after the first tool call.
        let mut cfg = runner_cfg(nats_port, mock_url, "default", api_port);
        cfg.max_iterations = 2;
        run(cfg).await.ok()
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Poll until the run completes (RunRecord = proof the agent finished).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The promise must reflect at least one checkpointed tool exchange.
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let (promise, _rev) = ps
        .get_promise("default", "github_pull_request.1.auto-multiturn")
        .await
        .expect("get_promise")
        .expect("promise must exist in KV");

    assert_eq!(promise.status, PromiseStatus::Resolved);
    assert!(
        promise.iteration >= 1,
        "promise must have at least one checkpointed iteration (got {})",
        promise.iteration
    );
    assert!(
        !promise.messages.is_empty(),
        "checkpoint must carry conversation messages"
    );
    // system_prompt is None when no memory is configured (correct behaviour —
    // it is stored as the actual system prompt used, which is None here because
    // runner_cfg does not set memory_owner/memory_repo). Covered by the
    // system_prompt_stored_in_first_checkpoint unit test in agent_loop.rs.
}

/// When Anthropic returns 5xx and all three retry attempts are exhausted, the
/// promise is marked `Failed` (not `PermanentFailed`) — indicating a transient
/// error that NATS redelivery may resolve.
#[tokio::test]
async fn dispatch_marks_promise_failed_on_transient_5xx_exhaustion() {
    use trogon_agent::promise_store::{PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&make_automation(
            "auto-5xx",
            "default",
            "github.pull_request",
            "5XX_PROMPT",
        ))
        .await
        .expect("put automation");

    // Anthropic always returns 500 — triggers the 2s+4s+8s retry backoff
    // (3 retries, ~14 s total) before the agent gives up.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(500).body("internal server error");
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // The retry backoff totals ~14 s; allow 25 s for the RunRecord to appear.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared after 5xx exhaustion within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let (promise, _rev) = ps
        .get_promise("default", "github_pull_request.1.auto-5xx")
        .await
        .expect("get_promise")
        .expect("promise must exist in KV");

    assert_eq!(
        promise.status,
        PromiseStatus::Failed,
        "exhausted 5xx retries must mark promise Failed (transient), not PermanentFailed"
    );
}

/// A Linear event that matches an automation produces a promise in KV with ID
/// `linear_issue.{seq}.{auto_id}` — verifies the promise_id prefix for the LINEAR
/// stream is distinct from the GITHUB prefix (`github_pull_request.{seq}.{auto_id}`).
#[tokio::test]
async fn linear_event_marks_promise_resolved_in_kv() {
    use trogon_agent::promise_store::{PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&make_automation(
            "auto-linear-kv",
            "default",
            "linear.Issue",
            "LINEAR_KV_PROMPT",
        ))
        .await
        .expect("put automation");

    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    // Publish a Linear Issue event — first message on the LINEAR stream → seq = 1.
    let payload = serde_json::to_vec(&json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-1", "title": "Integration test issue" }
    }))
    .unwrap();
    js.publish("linear.Issue.create", payload.into())
        .await
        .expect("publish");

    // Poll until RunRecord appears.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Promise ID = "linear_issue.{seq}.{auto_id}" = "linear_issue.1.auto-linear-kv".
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let (promise, _rev) = ps
        .get_promise("default", "linear_issue.1.auto-linear-kv")
        .await
        .expect("get_promise")
        .expect("promise must exist under linear_issue.1.auto-linear-kv");

    assert_eq!(promise.status, PromiseStatus::Resolved);
    assert_eq!(promise.automation_id, "auto-linear-kv");
    assert_eq!(
        promise.nats_subject, "linear.Issue",
        "runner normalises all Linear subjects to the canonical 'linear.Issue' string"
    );
}

// ── helpers for the remaining tests ──────────────────────────────────────────

/// Mirror of `agent_loop::tool_cache_key`.
///
/// Computes the SHA-256 hex digest used as the NATS KV key suffix when
/// caching a tool result.  Must stay in sync with the production
/// implementation in `src/agent_loop.rs`.
fn compute_tool_cache_key(tool_name: &str, input: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    fn sort_keys(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let sorted: serde_json::Map<String, serde_json::Value> = map
                    .iter()
                    .map(|(k, val)| (k.clone(), sort_keys(val)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect();
                serde_json::Value::Object(sorted)
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(sort_keys).collect())
            }
            other => other.clone(),
        }
    }
    let canonical = format!(
        "{tool_name}:{}",
        serde_json::to_string(&sort_keys(input)).unwrap_or_default()
    );
    let digest = Sha256::digest(canonical.as_bytes());
    format!("{digest:x}")
}

/// When a process crashes after executing a write tool (e.g. `create_pull_request`)
/// but before checkpointing the tool result into the promise messages, startup
/// recovery re-runs the automation.  The KV-cached tool result must be replayed —
/// the downstream API (GitHub) must NOT be called again.
///
/// Setup:
/// - Stale `Running` promise with non-empty `messages` checkpoint → `recovering = true`
/// - Pre-seeded tool result in `AGENT_TOOL_RESULTS` KV
///
/// The GitHub mock has no `header_exists` constraint and would match any POST to
/// the pulls endpoint.  A hit count of 0 after the run proves the cache was used.
#[tokio::test]
async fn tool_cache_dedup_on_recovery_skips_tool_reexecution() {
    use trogon_agent::agent_loop::Message;
    use trogon_agent::promise_store::{AgentPromise, PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&make_automation(
            "auto-cache",
            "default",
            "github.pull_request",
            "CACHE_DEDUP_PROMPT",
        ))
        .await
        .expect("put automation");

    // Pre-seed a stale Running promise with non-empty messages (simulates a crash
    // after the first LLM turn checkpointed but before a second tool call was
    // re-checkpointed). non-empty messages → `recovering = true` in `run()`.
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let stale_claimed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(20 * 60);
    let stale = AgentPromise {
        id: "github_pull_request.99.auto-cache".to_string(),
        tenant_id: "default".to_string(),
        automation_id: "auto-cache".to_string(),
        status: PromiseStatus::Running,
        // Non-empty → `recovering = true` so the KV cache is consulted.
        messages: vec![Message::user_text("trigger checkpoint")],
        iteration: 0,
        worker_id: "crashed-worker".to_string(),
        claimed_at: stale_claimed_at,
        trigger: serde_json::from_slice(&pr_opened_payload()).unwrap_or_default(),
        nats_subject: "github.pull_request".to_string(),
        system_prompt: None,
        recovery_count: 0,
        checkpoint_degraded: false,
        failure_reason: None,
    };
    ps.put_promise(&stale).await.expect("seed stale promise");

    // Pre-seed the tool result.  The Anthropic mock below returns a `create_pull_request`
    // tool_use with this exact input — the hash must match so the cache fires.
    let tool_input = serde_json::json!({
        "owner": "acme",
        "repo": "api",
        "title": "feat: cache test",
        "head": "feature/cache-test"
    });
    let cache_key = compute_tool_cache_key("create_pull_request", &tool_input);
    ps.put_tool_result(
        "default",
        "github_pull_request.99.auto-cache",
        &cache_key,
        "Pull request #42 opened: https://github.com/acme/api/pull/42",
    )
    .await
    .expect("seed tool result cache");

    // Second Anthropic call (body contains "tool_result") → end_turn. FIRST.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    // First Anthropic call (recovering from checkpoint) → create_pull_request tool_use.
    // The input must exactly match `tool_input` above so the hash aligns.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_cache_01","name":"create_pull_request","input":{"owner":"acme","repo":"api","title":"feat: cache test","head":"feature/cache-test"}}]}"#,
            );
    })
    .await;

    // GitHub mock — MUST NOT be hit when the cache replays the result.
    let github_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/github/repos/acme/api/pulls");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"number":99,"html_url":"https://github.com/acme/api/pull/99"}"#);
        })
        .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        let mut cfg = runner_cfg(nats_port, mock_url, "default", api_port);
        cfg.max_iterations = 2;
        run(cfg).await.ok()
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    // No event published — startup recovery finds the stale promise.
    // Allow up to 45 s: startup recovery jitter is 0–30 s, plus time for the
    // actual agent run. Tests waiting only 15 s would flake whenever jitter > 15 s.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared — startup recovery may have failed");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // GitHub hits = 0 → the KV cache suppressed re-execution.
    assert_eq!(
        github_mock.hits_async().await,
        0,
        "tool cache must suppress create_pull_request re-execution on recovery"
    );
}

/// When a promise is already `Resolved` (the original run completed successfully)
/// and NATS redelivers the same message — simulated by pre-seeding the `Resolved`
/// promise before publishing seq=1 — `prepare_agent_with_promise` must return
/// `None`, skipping the run entirely without calling the model.
///
/// A second "canary" automation on the same trigger fires normally and produces
/// a RunRecord, proving the event was processed before we assert the skip mock
/// was never hit.
#[tokio::test]
async fn resolved_promise_skips_redelivered_nats_message() {
    use trogon_agent::promise_store::{AgentPromise, PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    // "auto-skip": pre-seeded as Resolved → must be skipped on dispatch.
    astore
        .put(&make_automation(
            "auto-skip",
            "default",
            "github.pull_request",
            "SKIP_RESOLVED_PROMPT",
        ))
        .await
        .expect("put skip automation");
    // "auto-canary": no pre-seeded promise → runs normally, its RunRecord proves
    // the event was fully processed before the skip assertion is checked.
    astore
        .put(&make_automation(
            "auto-canary",
            "default",
            "github.pull_request",
            "CANARY_PROMPT",
        ))
        .await
        .expect("put canary automation");

    // Pre-seed the "skip" promise as Resolved.
    // Promise ID = "github_pull_request.1.auto-skip" — first message on GITHUB stream → seq=1.
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let resolved = AgentPromise {
        id: "github_pull_request.1.auto-skip".to_string(),
        tenant_id: "default".to_string(),
        automation_id: "auto-skip".to_string(),
        status: PromiseStatus::Resolved,
        messages: vec![],
        iteration: 1,
        worker_id: "prev-worker".to_string(),
        claimed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        trigger: serde_json::from_slice(&pr_opened_payload()).unwrap_or_default(),
        nats_subject: "github.pull_request".to_string(),
        system_prompt: None,
        recovery_count: 0,
        checkpoint_degraded: false,
        failure_reason: None,
    };
    ps.put_promise(&resolved).await.expect("seed resolved promise");

    // Canary mock — fires when "CANARY_PROMPT" is in the Anthropic request body.
    let canary_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("CANARY_PROMPT");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    // Skip mock — must NEVER be hit if the Resolved promise is correctly skipped.
    let skip_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("SKIP_RESOLVED_PROMPT");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Canary hit = 1 proves the event was processed and both automations were
    // considered (one ran, one was skipped).
    assert!(
        wait_for_hits(&canary_mock, 1, Duration::from_secs(10)).await,
        "canary automation must fire to confirm event was processed"
    );

    assert_eq!(
        skip_mock.hits_async().await,
        0,
        "Resolved promise must prevent re-execution on NATS redelivery"
    );
}

/// When two automations both match the same incoming event, `dispatch_automations`
/// runs them concurrently.  Each gets its own promise keyed
/// `{prefix}.{auto_id}` — they must not interfere and both must be `Resolved`.
#[tokio::test]
async fn two_automations_for_same_event_get_separate_resolved_promises() {
    use trogon_agent::promise_store::{PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&make_automation(
            "auto-dispatch-a",
            "default",
            "github.pull_request",
            "DISPATCH_A_PROMPT",
        ))
        .await
        .expect("put auto-a");
    astore
        .put(&make_automation(
            "auto-dispatch-b",
            "default",
            "github.pull_request",
            "DISPATCH_B_PROMPT",
        ))
        .await
        .expect("put auto-b");

    // Independent Anthropic mocks — matched by distinct body_contains constraints.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("DISPATCH_A_PROMPT");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("DISPATCH_B_PROMPT");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Poll until both RunRecords appear — dispatches run concurrently so order
    // is non-deterministic; we only care that count >= 2.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| a.len() >= 2).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let count = body.as_array().map(|a| a.len()).unwrap_or(0);
            panic!("Expected 2 RunRecords, got {count} within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Both promises must be Resolved with the correct automation_id.
    // First message on GITHUB stream → seq = 1 → prefix = "github_pull_request.1".
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let (pa, _) = ps
        .get_promise("default", "github_pull_request.1.auto-dispatch-a")
        .await
        .expect("get promise-a")
        .expect("promise-a must exist");
    let (pb, _) = ps
        .get_promise("default", "github_pull_request.1.auto-dispatch-b")
        .await
        .expect("get promise-b")
        .expect("promise-b must exist");

    assert_eq!(pa.status, PromiseStatus::Resolved, "promise-a must be Resolved");
    assert_eq!(pa.automation_id, "auto-dispatch-a");
    assert_eq!(pb.status, PromiseStatus::Resolved, "promise-b must be Resolved");
    assert_eq!(pb.automation_id, "auto-dispatch-b");
}

/// The runner's HTTP server exposes the full automation lifecycle management API.
/// Tests the complete CRUD flow via HTTP: create → read → update → disable →
/// enable → delete, verifying status codes and response bodies at each step.
#[tokio::test]
async fn automations_http_api_crud_lifecycle() {
    let (_c, nats_port) = start_nats().await;
    // No Anthropic calls expected — this test only exercises the management API.
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{api_port}");

    // Wait for API.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match client
            .get(format!("{base}/automations"))
            .header("x-tenant-id", "default")
            .send()
            .await
        {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    // 1. GET /automations → empty initially.
    let list: serde_json::Value = client
        .get(format!("{base}/automations"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 0, "initially empty");

    // 2. POST /automations → 201 Created.
    let create_resp = client
        .post(format!("{base}/automations"))
        .header("x-tenant-id", "default")
        .json(&serde_json::json!({
            "name": "CRUD Automation",
            "trigger": "github.pull_request",
            "prompt": "CRUD test prompt"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), 201);
    let created: serde_json::Value = create_resp.json().await.unwrap();
    let auto_id = created["id"].as_str().expect("id must be present").to_string();
    assert_eq!(created["name"], "CRUD Automation");
    assert!(created["enabled"].as_bool().unwrap_or(false), "enabled by default");

    // 3. GET /automations/:id → round-trip.
    let fetched: serde_json::Value = client
        .get(format!("{base}/automations/{auto_id}"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fetched["id"].as_str().unwrap(), auto_id);
    assert_eq!(fetched["prompt"], "CRUD test prompt");

    // 4. PUT /automations/:id → replace name and prompt.
    let updated: serde_json::Value = client
        .put(format!("{base}/automations/{auto_id}"))
        .header("x-tenant-id", "default")
        .json(&serde_json::json!({
            "name": "Updated Automation",
            "trigger": "github.pull_request",
            "prompt": "Updated prompt",
            "enabled": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["name"], "Updated Automation");
    assert_eq!(updated["prompt"], "Updated prompt");

    // 5. PATCH /automations/:id/disable → 200, enabled = false.
    let disabled: serde_json::Value = client
        .patch(format!("{base}/automations/{auto_id}/disable"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!disabled["enabled"].as_bool().unwrap_or(true), "must be disabled");

    // 6. PATCH /automations/:id/enable → 200, enabled = true.
    let re_enabled: serde_json::Value = client
        .patch(format!("{base}/automations/{auto_id}/enable"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(re_enabled["enabled"].as_bool().unwrap_or(false), "must be re-enabled");

    // 7. DELETE /automations/:id → 204 No Content.
    let del_status = client
        .delete(format!("{base}/automations/{auto_id}"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(del_status, 204);

    // 8. GET /automations/:id → 404 after deletion.
    let not_found = client
        .get(format!("{base}/automations/{auto_id}"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(not_found, 404);

    // 9. GET /automations → empty again after deletion.
    let final_list: serde_json::Value = client
        .get(format!("{base}/automations"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(final_list.as_array().unwrap().len(), 0, "empty after delete");
}

/// The `_idempotency_key` injected into built-in tool inputs must be forwarded
/// as an `Idempotency-Key` HTTP header on write calls.  The GitHub mock uses
/// `header_exists("Idempotency-Key")` so it only matches when the header is
/// present — a hit count of 1 proves the header reached the downstream API
/// end-to-end through the durable promise pipeline.
#[tokio::test]
async fn write_tool_sends_idempotency_key_header() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    astore
        .put(&make_automation(
            "auto-idem",
            "default",
            "github.pull_request",
            "IDEM_PROMPT",
        ))
        .await
        .expect("put automation");

    // Second Anthropic call (body contains "tool_result") → end_turn.
    // Registered FIRST so httpmock's FIFO ordering handles the multi-turn flow;
    // the body_contains constraint prevents it from matching the initial call.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    // First Anthropic call → create_pull_request tool_use.
    // Registered SECOND (broader match — fires for the initial call which has
    // no tool_result block in the body yet).
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tu_idem_01","name":"create_pull_request","input":{"owner":"acme","repo":"api","title":"feat: idem test","head":"feature/idem-test"}}]}"#,
            );
    })
    .await;

    // GitHub PR endpoint — only matches when the Idempotency-Key header is present.
    // If key injection is broken (header absent), this mock never fires → hits = 0
    // and the final assertion fails.
    let github_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/github/repos/acme/api/pulls")
                .header_exists("Idempotency-Key");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"number":99,"html_url":"https://github.com/acme/api/pull/99"}"#);
        })
        .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        // max_iterations = 2 so the second LLM call (after tool result) reaches
        // end_turn; the default of 1 would terminate after the first tool call.
        let mut cfg = runner_cfg(nats_port, mock_url, "default", api_port);
        cfg.max_iterations = 2;
        run(cfg).await.ok()
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
            Err(e) => panic!("API never came up: {e}"),
        }
    }

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Wait for the run to complete (RunRecord is proof the agent finished).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let body: serde_json::Value = client
            .get(&runs_url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared within timeout");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // hits = 1 → the Idempotency-Key header was forwarded correctly end-to-end.
    assert_eq!(
        github_mock.hits_async().await,
        1,
        "create_pull_request must send exactly one request with Idempotency-Key header"
    );
}

/// Variables defined on an automation are substituted for `{{key}}` placeholders
/// in the prompt before the request is forwarded to the model.
///
/// Creates an automation with `prompt: "Deploy {{repo}} to {{env}}"` and
/// `variables: {"repo": "api", "env": "prod"}`, then asserts that the Anthropic
/// request body contains the resolved string `"Deploy api to prod"` — not the
/// raw placeholders.
#[tokio::test]
async fn automation_variables_substituted_in_prompt_before_model_call() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");

    let mut vars = std::collections::HashMap::new();
    vars.insert("repo".to_string(), "api".to_string());
    vars.insert("env".to_string(), "prod".to_string());

    store
        .put(&trogon_automations::Automation {
            id: "auto-vars".to_string(),
            tenant_id: "default".to_string(),
            name: "Var substitution test".to_string(),
            trigger: "github.pull_request".to_string(),
            prompt: "Deploy {{repo}} to {{env}}".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            mcp_servers: vec![],
            enabled: true,
            visibility: trogon_automations::Visibility::Private,
            variables: vars,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            skill_ids: vec![],
        })
        .await
        .expect("put automation");

    // Match only when the substituted text appears — placeholders replaced.
    let substituted_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Deploy api to prod");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    // If the raw placeholders appear instead, the substitution is broken.
    let raw_placeholder_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("{{repo}}");
            then.status(500).body("substitution did not happen");
        })
        .await;

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", 0))
            .await
            .ok()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&substituted_mock, 1, Duration::from_secs(8)).await,
        "substituted prompt 'Deploy api to prod' must reach the model"
    );
    assert_eq!(
        raw_placeholder_mock.hits_async().await,
        0,
        "raw {{repo}} placeholder must not appear in the Anthropic request"
    );
}

// ── Helper: wait for the HTTP API to respond ──────────────────────────────────

async fn wait_for_api(client: &reqwest::Client, url: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match client.get(url).header("x-tenant-id", "default").send().await {
            Ok(_) => return,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("API never came up: {e}"),
        }
    }
}

/// Poll GET `url` until the JSON array is non-empty, then return it.
async fn poll_runs_until_nonempty(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let body: serde_json::Value = client
            .get(url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .expect("GET runs")
            .json()
            .await
            .expect("json");
        if body.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            return body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("No RunRecord appeared within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Poll GET `url` until `predicate` is satisfied, then return the body.
async fn poll_runs_until<F>(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
    predicate: F,
) -> serde_json::Value
where
    F: Fn(&serde_json::Value) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let body: serde_json::Value = client
            .get(url)
            .header("x-tenant-id", "default")
            .send()
            .await
            .expect("GET runs")
            .json()
            .await
            .expect("json");
        if predicate(&body) {
            return body;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("Predicate not satisfied within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ── New integration tests ──────────────────────────────────────────────────────

/// Full HTTP lifecycle: automation created via POST /automations → NATS event
/// triggered → RunRecord appears in GET /runs → GET /stats reflects the count.
///
/// This is the only test that creates the automation through the HTTP API
/// (instead of directly via AutomationStore), exercising the complete path a
/// real user would follow.
#[tokio::test]
async fn full_http_lifecycle_automation_created_via_http() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("HTTP_LIFECYCLE_PROMPT");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{api_port}");
    wait_for_api(&client, &format!("{base}/automations")).await;

    // Create automation via HTTP (not via AutomationStore directly).
    let created: serde_json::Value = client
        .post(format!("{base}/automations"))
        .header("x-tenant-id", "default")
        .json(&serde_json::json!({
            "name": "HTTP Lifecycle Test",
            "trigger": "github.pull_request",
            "prompt": "HTTP_LIFECYCLE_PROMPT"
        }))
        .send()
        .await
        .expect("POST /automations")
        .json()
        .await
        .expect("json");
    assert_eq!(created["enabled"], true);
    let auto_id = created["id"].as_str().expect("id").to_string();

    // Trigger the event.
    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Wait for RunRecord to appear via GET /runs.
    let runs =
        poll_runs_until_nonempty(&client, &format!("{base}/runs"), Duration::from_secs(10)).await;
    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 1, "exactly one run");
    assert_eq!(arr[0]["automation_id"], auto_id.as_str());
    assert_eq!(arr[0]["status"], "success");
    assert_eq!(arr[0]["tenant_id"], "default");

    // Verify GET /stats reflects the run.
    let stats: serde_json::Value = client
        .get(format!("{base}/stats"))
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

/// When NATS redelivers a message whose promise is already `Resolved`,
/// the run is skipped — but GET /runs must show exactly 1 RunRecord, not 2.
///
/// Extends `resolved_promise_skips_redelivered_nats_message` to verify that
/// the skip does not produce a duplicate record in the persistent store.
#[tokio::test]
async fn resolved_promise_redelivery_produces_single_run_record() {
    use trogon_agent::promise_store::{AgentPromise, PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let astore = AutomationStore::open(&js).await.expect("open AutomationStore");
    // "auto-skip": pre-seeded as Resolved → skipped, must not produce a RunRecord.
    astore
        .put(&make_automation(
            "auto-skip-idem",
            "default",
            "github.pull_request",
            "SKIP_IDEM_PROMPT",
        ))
        .await
        .expect("put skip automation");
    // "auto-canary": runs normally, its RunRecord proves the event was processed.
    astore
        .put(&make_automation(
            "auto-canary-idem",
            "default",
            "github.pull_request",
            "CANARY_IDEM_PROMPT",
        ))
        .await
        .expect("put canary automation");

    // Pre-seed the skip promise as Resolved (seq=1 on GITHUB stream).
    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");
    let resolved = AgentPromise {
        id: "github_pull_request.1.auto-skip-idem".to_string(),
        tenant_id: "default".to_string(),
        automation_id: "auto-skip-idem".to_string(),
        status: PromiseStatus::Resolved,
        messages: vec![],
        iteration: 1,
        worker_id: "prev-worker".to_string(),
        claimed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        trigger: serde_json::from_slice(&pr_opened_payload()).unwrap_or_default(),
        nats_subject: "github.pull_request".to_string(),
        system_prompt: None,
        recovery_count: 0,
        checkpoint_degraded: false,
        failure_reason: None,
    };
    ps.put_promise(&resolved).await.expect("seed resolved promise");

    // Canary mock — proves the event was processed.
    let canary_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("CANARY_IDEM_PROMPT");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;
    // Catch-all so skip automation doesn't hang if accidentally called.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{api_port}");
    wait_for_api(&client, &format!("{base}/automations")).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Wait until canary run appears — proves the event was fully processed.
    assert!(
        wait_for_hits(&canary_mock, 1, Duration::from_secs(10)).await,
        "canary must fire to confirm event was processed"
    );

    // Give the runner a moment to write the RunRecord, then assert exactly 1.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let runs: serde_json::Value = client
        .get(format!("{base}/runs"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = runs.as_array().unwrap();
    assert_eq!(
        arr.len(),
        1,
        "skipped automation must not produce a RunRecord; expected 1 (canary), got {arr:?}"
    );
    assert_eq!(arr[0]["automation_id"], "auto-canary-idem");
}

/// The `output` field of a RunRecord must contain the final text the agent
/// produced — i.e. the text from the last `end_turn` model response.
#[tokio::test]
async fn run_record_output_contains_agent_final_text() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-output",
            "default",
            "github.pull_request",
            "OUTPUT_CAPTURE_PROMPT",
        ))
        .await
        .expect("put automation");

    // Return a distinctive final text so we can assert it lands in RunRecord.output.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"DISTINCTIVE_FINAL_OUTPUT"}]}"#);
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{api_port}");
    wait_for_api(&client, &format!("{base}/automations")).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    let runs =
        poll_runs_until_nonempty(&client, &format!("{base}/runs"), Duration::from_secs(10)).await;
    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let output = arr[0]["output"].as_str().unwrap_or("");
    assert!(
        output.contains("DISTINCTIVE_FINAL_OUTPUT"),
        "RunRecord.output must contain the agent's final text; got: {output:?}"
    );
}

/// When two automations both match the same incoming event, both run concurrently
/// and both produce a RunRecord. GET /runs must return 2 records; GET /stats
/// must reflect total=2, successful_7d=2.
#[tokio::test]
async fn two_concurrent_automations_both_appear_in_runs_and_stats() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-concurrent-A",
            "default",
            "github.pull_request",
            "CONCURRENT_PROMPT_A",
        ))
        .await
        .expect("put A");
    store
        .put(&make_automation(
            "auto-concurrent-B",
            "default",
            "github.pull_request",
            "CONCURRENT_PROMPT_B",
        ))
        .await
        .expect("put B");

    // Both prompts hit the same catch-all mock and return successfully.
    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{api_port}");
    wait_for_api(&client, &format!("{base}/automations")).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Wait until both RunRecords appear (both automations must complete).
    let runs = poll_runs_until(
        &client,
        &format!("{base}/runs"),
        Duration::from_secs(12),
        |body| body.as_array().map(|a| a.len() >= 2).unwrap_or(false),
    )
    .await;
    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 2, "both automations must produce a RunRecord");

    let ids: Vec<&str> = arr
        .iter()
        .map(|r| r["automation_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"auto-concurrent-A"), "RunRecord for A missing");
    assert!(ids.contains(&"auto-concurrent-B"), "RunRecord for B missing");
    assert!(
        arr.iter().all(|r| r["status"] == "success"),
        "both runs must succeed"
    );

    // Stats must reflect both runs.
    let stats: serde_json::Value = client
        .get(format!("{base}/stats"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats["total"], 2);
    assert_eq!(stats["successful_7d"], 2);
    assert_eq!(stats["failed_7d"], 0);
}

/// GET /automations/{id}/runs returns only runs for that automation.
///
/// Two automations share the same trigger; after a single event both produce a
/// RunRecord. The per-automation endpoint must filter correctly so that each
/// automation sees exactly its own record and not the other's.
#[tokio::test]
async fn automation_filtered_runs_endpoint_returns_only_matching_automation() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-filter-A",
            "default",
            "github.pull_request",
            "FILTER_PROMPT_A",
        ))
        .await
        .expect("put A");
    store
        .put(&make_automation(
            "auto-filter-B",
            "default",
            "github.pull_request",
            "FILTER_PROMPT_B",
        ))
        .await
        .expect("put B");

    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{api_port}");
    wait_for_api(&client, &format!("{base}/automations")).await;

    js.publish("github.pull_request", pr_opened_payload().into())
        .await
        .expect("publish");

    // Wait until both RunRecords appear in the global /runs endpoint.
    poll_runs_until(
        &client,
        &format!("{base}/runs"),
        Duration::from_secs(12),
        |body| body.as_array().map(|a| a.len() >= 2).unwrap_or(false),
    )
    .await;

    // /automations/auto-filter-A/runs must return only A's run.
    let runs_a: serde_json::Value = client
        .get(format!("{base}/automations/auto-filter-A/runs"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .expect("GET /automations/auto-filter-A/runs")
        .json()
        .await
        .expect("json");
    let arr_a = runs_a.as_array().expect("array");
    assert_eq!(arr_a.len(), 1, "/automations/auto-filter-A/runs must return exactly 1 record");
    assert_eq!(arr_a[0]["automation_id"], "auto-filter-A");
    assert_eq!(arr_a[0]["status"], "success");

    // /automations/auto-filter-B/runs must return only B's run.
    let runs_b: serde_json::Value = client
        .get(format!("{base}/automations/auto-filter-B/runs"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .expect("GET /automations/auto-filter-B/runs")
        .json()
        .await
        .expect("json");
    let arr_b = runs_b.as_array().expect("array");
    assert_eq!(arr_b.len(), 1, "/automations/auto-filter-B/runs must return exactly 1 record");
    assert_eq!(arr_b[0]["automation_id"], "auto-filter-B");
    assert_eq!(arr_b[0]["status"], "success");
}

/// A CRON tick dispatched to a matching automation produces a RunRecord that is
/// accessible via GET /runs and reflected in GET /stats.
#[tokio::test]
async fn cron_tick_produces_run_record_accessible_via_api() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (nats, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-cron-runs",
            "default",
            "cron.daily",
            "CRON_RUNS_PROMPT",
        ))
        .await
        .expect("put cron automation");

    mock.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("CRON_RUNS_PROMPT");
        then.status(200)
            .header("content-type", "application/json")
            .body(end_turn_body());
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg_with_cron(
            nats_port,
            mock_url,
            "default",
            api_port,
            Some("CRON_TICKS".to_string()),
        ))
        .await
        .ok()
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{api_port}");
    wait_for_api(&client, &format!("{base}/automations")).await;

    nats.publish("cron.daily", b"{}".as_slice().into())
        .await
        .expect("publish cron tick");

    let runs = poll_runs_until_nonempty(
        &client,
        &format!("{base}/runs"),
        Duration::from_secs(12),
    )
    .await;

    let arr = runs.as_array().unwrap();
    assert_eq!(arr.len(), 1, "exactly one RunRecord expected for cron tick");
    assert_eq!(arr[0]["automation_id"], "auto-cron-runs");
    assert_eq!(arr[0]["status"], "success");
    assert_eq!(arr[0]["nats_subject"], "cron.daily");
    assert_eq!(arr[0]["tenant_id"], "default");

    let stats: serde_json::Value = client
        .get(format!("{base}/stats"))
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

/// `GET /admin/promises` returns Running promises visible to operators and
/// excludes terminal ones.
///
/// Strategy:
/// 1. Inject a Running promise directly into PromiseStore (real NATS KV).
/// 2. Verify the endpoint returns it with the correct public fields.
/// 3. Verify that `messages` and `system_prompt` are excluded from the view.
/// 4. Mark the promise PermanentFailed via PromiseStore.
/// 5. Verify the endpoint now returns an empty array.
#[tokio::test]
async fn admin_promises_endpoint_returns_running_and_excludes_terminal() {
    use trogon_agent::promise_store::{AgentPromise, PromiseStatus, PromiseStore};

    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let ps = PromiseStore::open(&js).await.expect("open PromiseStore");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_port = listener.local_addr().unwrap().port();
    drop(listener);

    tokio::spawn(async move {
        run(runner_cfg(nats_port, mock_url, "default", api_port))
            .await
            .ok()
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{api_port}");
    wait_for_api(&client, &format!("{base}/automations")).await;

    // ── 1. Inject a Running promise ───────────────────────────────────────────
    let promise_id = "github_pull_request.99.auto-admin-test";
    ps.put_promise(&AgentPromise {
        id: promise_id.to_string(),
        tenant_id: "default".to_string(),
        automation_id: "auto-admin-test".to_string(),
        status: PromiseStatus::Running,
        messages: vec![],
        iteration: 2,
        worker_id: "worker-abc".to_string(),
        claimed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        trigger: serde_json::json!({"action": "opened"}),
        nats_subject: "github.pull_request".to_string(),
        system_prompt: Some("secret system prompt".to_string()),
        recovery_count: 0,
        checkpoint_degraded: false,
        failure_reason: None,
    })
    .await
    .expect("seed Running promise");

    // ── 2. Verify the endpoint returns it ─────────────────────────────────────
    let body: serde_json::Value = client
        .get(format!("{base}/admin/promises"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .expect("GET /admin/promises")
        .json()
        .await
        .expect("json");

    let arr = body.as_array().expect("expected JSON array");
    assert_eq!(arr.len(), 1, "one Running promise must appear");

    let p = &arr[0];
    assert_eq!(p["id"], promise_id);
    assert_eq!(p["automation_id"], "auto-admin-test");
    assert_eq!(p["status"], "running");
    assert_eq!(p["nats_subject"], "github.pull_request");
    assert_eq!(p["iteration"], 2);
    assert_eq!(p["worker_id"], "worker-abc");
    assert_eq!(p["recovery_count"], 0);

    // ── 3. Sensitive fields must be absent from the view ──────────────────────
    assert!(
        p.get("messages").is_none(),
        "`messages` must be excluded from /admin/promises view"
    );
    assert!(
        p.get("system_prompt").is_none(),
        "`system_prompt` must be excluded from /admin/promises view"
    );

    // ── 4. Mark promise terminal ──────────────────────────────────────────────
    let (mut updated, rev) = ps
        .get_promise("default", promise_id)
        .await
        .expect("get")
        .expect("promise exists");
    updated.status = PromiseStatus::PermanentFailed;
    updated.failure_reason = Some("test cleanup".to_string());
    ps.update_promise("default", promise_id, &updated, rev)
        .await
        .expect("mark terminal");

    // ── 5. Terminal promise must not appear ───────────────────────────────────
    let after: serde_json::Value = client
        .get(format!("{base}/admin/promises"))
        .header("x-tenant-id", "default")
        .send()
        .await
        .expect("GET /admin/promises after terminal")
        .json()
        .await
        .expect("json");

    assert_eq!(
        after,
        serde_json::json!([]),
        "terminal promise must not appear in /admin/promises"
    );
}
