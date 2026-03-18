//! E2e automation dispatch for the three event types not covered in
//! automation_dispatch_e2e.rs / automation_dispatch_gaps.rs:
//!
//! 1. `github.push`         → automation dispatch (push_to_branch fallback otherwise)
//! 2. `github.issue_comment`→ automation dispatch (comment_added fallback otherwise)
//! 3. `github.check_run`    → automation dispatch (ci_completed fallback otherwise)
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test automation_other_events_e2e -- --test-threads=1

use std::time::Duration;

use async_nats::jetstream;
use httpmock::MockServer;
use serde_json::json;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent::{AgentConfig, run};
use trogon_automations::{Automation, AutomationStore};
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
    for (name, subject) in [("GITHUB", "github.>"), ("LINEAR", "linear.Issue.>"), ("CRON_TICKS", "cron.>")] {
        js.get_or_create_stream(jetstream::stream::Config {
            name: name.to_string(),
            subjects: vec![subject.to_string()],
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("create stream {name}: {e}"));
    }
}

fn runner_cfg(nats_port: u16, proxy_url: String) -> AgentConfig {
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
    }
}

fn make_automation(id: &str, trigger: &str, prompt: &str) -> Automation {
    Automation {
        id: id.to_string(),
        tenant_id: "default".to_string(),
        name: format!("Test {id}"),
        trigger: trigger.to_string(),
        prompt: prompt.to_string(),
        model: None,
        tools: vec![],
        memory_path: None,
        mcp_servers: vec![],
        enabled: true,
        visibility: trogon_automations::Visibility::Private,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn end_turn_body() -> &'static str {
    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}"#
}

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

/// A `github.push` event dispatches to a matching automation.
#[tokio::test]
async fn push_event_dispatches_to_automation() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation("auto-push", "github.push", "PUSH_AUTOMATION_PROMPT_XYZ"))
        .await
        .expect("put automation");

    let anthropic = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("PUSH_AUTOMATION_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "ref": "refs/heads/main",
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pusher": { "name": "alice" },
        "commits": [{ "id": "abc123", "message": "Fix bug" }]
    }))
    .unwrap();
    js.publish("github.push", payload.into()).await.expect("publish");

    assert!(
        wait_for_hits(&anthropic, 1, Duration::from_secs(8)).await,
        "push automation was not dispatched"
    );
}

/// When no automation matches a `github.push` event, the hardcoded
/// `push_to_branch` handler runs.
#[tokio::test]
async fn push_event_falls_back_to_hardcoded_handler() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;
    // No automations stored.

    // push_to_branch uses "commit(s) to branch" in its prompt.
    let fallback_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("commit(s) to branch");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "ref": "refs/heads/feature",
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pusher": { "name": "alice" },
        "commits": [{ "id": "def456", "message": "Add feature" }]
    }))
    .unwrap();
    js.publish("github.push", payload.into()).await.expect("publish");

    assert!(
        wait_for_hits(&fallback_mock, 1, Duration::from_secs(8)).await,
        "fallback push_to_branch handler was not called"
    );
}

/// A `github.issue_comment` event dispatches to a matching automation.
#[tokio::test]
async fn issue_comment_event_dispatches_to_automation() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-comment",
            "github.issue_comment",
            "COMMENT_AUTOMATION_PROMPT_XYZ",
        ))
        .await
        .expect("put automation");

    let anthropic = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("COMMENT_AUTOMATION_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "created",
        "issue": { "number": 7, "title": "Login bug" },
        "comment": { "body": "Can you add a test?" },
        "repository": { "owner": { "login": "acme" }, "name": "api" }
    }))
    .unwrap();
    js.publish("github.issue_comment", payload.into()).await.expect("publish");

    assert!(
        wait_for_hits(&anthropic, 1, Duration::from_secs(8)).await,
        "issue_comment automation was not dispatched"
    );
}

/// When no automation matches a `github.issue_comment`, the hardcoded
/// `comment_added` handler runs.
#[tokio::test]
async fn issue_comment_event_falls_back_to_hardcoded_handler() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;
    // No automations stored.

    // comment_added uses "commented on" in its prompt.
    let fallback_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("commented on");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "created",
        "issue": { "number": 8, "title": "Bug" },
        "comment": { "body": "Please fix" },
        "repository": { "owner": { "login": "acme" }, "name": "api" }
    }))
    .unwrap();
    js.publish("github.issue_comment", payload.into()).await.expect("publish");

    assert!(
        wait_for_hits(&fallback_mock, 1, Duration::from_secs(8)).await,
        "fallback comment_added handler was not called"
    );
}

/// A `github.check_run` event dispatches to a matching automation.
#[tokio::test]
async fn check_run_event_dispatches_to_automation() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-ci",
            "github.check_run",
            "CI_AUTOMATION_PROMPT_XYZ",
        ))
        .await
        .expect("put automation");

    let anthropic = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("CI_AUTOMATION_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "completed",
        "check_run": {
            "name": "tests",
            "conclusion": "failure",
            "head_sha": "abc123"
        },
        "repository": { "owner": { "login": "acme" }, "name": "api" }
    }))
    .unwrap();
    js.publish("github.check_run", payload.into()).await.expect("publish");

    assert!(
        wait_for_hits(&anthropic, 1, Duration::from_secs(8)).await,
        "check_run automation was not dispatched"
    );
}

/// When no automation matches `github.check_run` with conclusion=failure,
/// the hardcoded `ci_completed` handler runs.
#[tokio::test]
async fn check_run_failure_falls_back_to_hardcoded_handler() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;
    // No automations stored.

    // ci_completed uses "CI check" in its prompt.
    let fallback_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("CI check");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url)).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "completed",
        "check_run": {
            "name": "tests",
            "conclusion": "failure",
            "head_sha": "def456"
        },
        "repository": { "owner": { "login": "acme" }, "name": "api" }
    }))
    .unwrap();
    js.publish("github.check_run", payload.into()).await.expect("publish");

    assert!(
        wait_for_hits(&fallback_mock, 1, Duration::from_secs(8)).await,
        "fallback ci_completed handler was not called"
    );
}
