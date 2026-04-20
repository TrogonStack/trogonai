//! Gap tests for the automation dispatch pipeline.
//!
//! Covers scenarios not exercised in automation_dispatch_e2e.rs:
//!
//! 1. Action-filtered trigger fires only when the action matches.
//! 2. Action-filtered trigger does NOT fire for a different action → fallback.
//! 3. Tenant isolation: automation stored for tenant A ignored by runner for tenant B.
//! 4. Linear Issue event dispatches to a matching automation.
//! 5. Two matching automations both execute for a single event.
//! 6. PR-merged event dispatches to automation (not hardcoded pr_merged handler).
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test automation_dispatch_gaps -- --test-threads=1

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

fn runner_cfg(nats_port: u16, proxy_url: String, tenant_id: &str) -> AgentConfig {
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
        api_port: 0,
        tenant_id: tenant_id.to_string(),
        incidentio_stream_name: None,
        split_evaluator_url: None,
        split_auth_token: None,
        agent_id: None,
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
        skill_ids: vec![],
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

/// Automation with `trigger = "github.pull_request:opened"` fires when the
/// incoming event has `action = "opened"`.
#[tokio::test]
async fn action_filtered_trigger_fires_on_matching_action() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-opened",
            "default",
            "github.pull_request:opened",
            "ACTION_FILTERED_PROMPT_XYZ",
        ))
        .await
        .expect("put automation");

    let anthropic = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("ACTION_FILTERED_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 10,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "Add feature" }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&anthropic, 1, Duration::from_secs(8)).await,
        "action-filtered automation was not called for matching action"
    );
}

/// Automation with `trigger = "github.pull_request:opened"` does NOT fire
/// when the incoming event has a different action — the fallback handler runs.
#[tokio::test]
async fn action_filtered_trigger_skips_non_matching_action() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    // Trigger only fires on "opened"; we'll send "reopened" — still a REVIEW_ACTION
    // so pr_review fallback will run if automation correctly doesn't match.
    store
        .put(&make_automation(
            "auto-opened-only",
            "default",
            "github.pull_request:opened",
            "SHOULD_NOT_APPEAR_IN_REQUEST",
        ))
        .await
        .expect("put automation");

    // If automation fires → 500 (catches wrong dispatch).
    let bad_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("SHOULD_NOT_APPEAR_IN_REQUEST");
            then.status(500)
                .body("automation must not fire for non-matching action");
        })
        .await;

    // Fallback pr_review contains "code reviewer".
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

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Send "reopened" — does not match ":opened" filter.
    let payload = serde_json::to_vec(&json!({
        "action": "reopened",
        "number": 11,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "Old feature" }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&fallback_mock, 1, Duration::from_secs(8)).await,
        "fallback handler was not called when action didn't match automation trigger"
    );
    assert_eq!(
        bad_mock.hits_async().await,
        0,
        "automation fired for wrong action"
    );
}

/// An automation stored for tenant "acme" must NOT run when the runner
/// is configured for a different tenant ("default").
#[tokio::test]
async fn tenant_isolation_automation_does_not_fire_for_wrong_tenant() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    // Stored for "acme" but runner uses "default".
    store
        .put(&make_automation(
            "auto-acme",
            "acme",
            "github.pull_request",
            "ACME_TENANT_PROMPT_XYZ",
        ))
        .await
        .expect("put automation");

    // Wrong-tenant automation fires → 500.
    let bad_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("ACME_TENANT_PROMPT_XYZ");
            then.status(500)
                .body("cross-tenant automation must not fire");
        })
        .await;

    // Fallback for "default" tenant (no matching automations).
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

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 20,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "Cross-tenant PR" }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&fallback_mock, 1, Duration::from_secs(8)).await,
        "fallback did not run — wrong-tenant automation may have matched"
    );
    assert_eq!(
        bad_mock.hits_async().await,
        0,
        "cross-tenant automation fired"
    );
}

/// A Linear Issue event dispatches to a matching automation.
#[tokio::test]
async fn linear_event_dispatches_to_automation() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-linear",
            "default",
            "linear.Issue",
            "LINEAR_AUTOMATION_PROMPT_XYZ",
        ))
        .await
        .expect("put automation");

    let anthropic = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("LINEAR_AUTOMATION_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-99", "title": "Bug in login" }
    }))
    .unwrap();
    js.publish("linear.Issue.create", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&anthropic, 1, Duration::from_secs(8)).await,
        "linear automation was not dispatched"
    );
}

/// Two automations that match the same event both execute (parallel dispatch).
#[tokio::test]
async fn two_matching_automations_both_execute() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-p1",
            "default",
            "github.pull_request",
            "PARALLEL_PROMPT_ALPHA",
        ))
        .await
        .expect("put automation 1");
    store
        .put(&make_automation(
            "auto-p2",
            "default",
            "github.pull_request",
            "PARALLEL_PROMPT_BETA",
        ))
        .await
        .expect("put automation 2");

    let mock_a = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("PARALLEL_PROMPT_ALPHA");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    let mock_b = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("PARALLEL_PROMPT_BETA");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 30,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "Parallel test" }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&mock_a, 1, Duration::from_secs(8)).await,
        "first automation (ALPHA) was not called"
    );
    assert!(
        wait_for_hits(&mock_b, 1, Duration::from_secs(8)).await,
        "second automation (BETA) was not called"
    );
}

/// When a PR-merged event arrives and a matching automation exists, the
/// automation runs — the hardcoded pr_merged handler is NOT called.
#[tokio::test]
async fn pr_merged_event_dispatches_to_automation_not_hardcoded_handler() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-merged",
            "default",
            "github.pull_request",
            "MERGED_AUTOMATION_PROMPT_XYZ",
        ))
        .await
        .expect("put automation");

    // pr_merged handler uses "was just merged by" in its prompt.
    let bad_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("was just merged by");
            then.status(500)
                .body("hardcoded pr_merged handler must not run");
        })
        .await;

    let auto_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("MERGED_AUTOMATION_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "closed",
        "number": 40,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": {
            "title": "Merged feature",
            "merged": true,
            "merged_by": { "login": "alice" }
        }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&auto_mock, 1, Duration::from_secs(8)).await,
        "automation was not called for merged PR event"
    );
    assert_eq!(
        bad_mock.hits_async().await,
        0,
        "hardcoded pr_merged handler ran despite matching automation"
    );
}

/// An automation with trigger `github.pull_request:draft_opened` fires when a
/// PR arrives with `action=opened` AND `pull_request.draft=true`.
#[tokio::test]
async fn draft_opened_trigger_fires_for_draft_pr() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-draft",
            "default",
            "github.pull_request:draft_opened",
            "DRAFT_PR_AUTOMATION_PROMPT",
        ))
        .await
        .expect("put automation");

    let draft_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("DRAFT_PR_AUTOMATION_PROMPT");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Draft PR: action=opened + pull_request.draft=true.
    let payload = serde_json::to_vec(&serde_json::json!({
        "action": "opened",
        "number": 99,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "WIP feature", "draft": true }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&draft_mock, 1, Duration::from_secs(8)).await,
        "draft_opened automation was not dispatched for draft PR"
    );
}

/// `draft_opened` automation does NOT fire for a non-draft PR that is opened.
#[tokio::test]
async fn draft_opened_trigger_does_not_fire_for_non_draft_pr() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-draft-only",
            "default",
            "github.pull_request:draft_opened",
            "SHOULD_NOT_FIRE_FOR_NON_DRAFT",
        ))
        .await
        .expect("put automation");

    // If draft automation fires → 500.
    let bad_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("SHOULD_NOT_FIRE_FOR_NON_DRAFT");
            then.status(500)
                .body("draft_opened must not fire for non-draft PR");
        })
        .await;

    // Fallback pr_review contains "code reviewer".
    let fallback = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("code reviewer");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Non-draft PR: action=opened + pull_request.draft=false.
    let payload = serde_json::to_vec(&serde_json::json!({
        "action": "opened",
        "number": 100,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "Real feature", "draft": false }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&fallback, 1, Duration::from_secs(8)).await,
        "fallback handler not called for non-draft PR"
    );
    assert_eq!(
        bad_mock.hits_async().await,
        0,
        "draft_opened fired for non-draft PR"
    );
}

/// An automation with trigger `github.pull_request:pushed` fires when a PR
/// event arrives with `action=synchronize` (GitHub's label for "pushed commits").
#[tokio::test]
async fn pushed_trigger_fires_on_synchronize_action() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-pushed",
            "default",
            "github.pull_request:pushed",
            "PUSHED_AUTOMATION_PROMPT_XYZ",
        ))
        .await
        .expect("put automation");

    let anthropic = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("PUSHED_AUTOMATION_PROMPT_XYZ");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    // GitHub sends action="synchronize" when new commits are pushed to an open PR.
    let payload = serde_json::to_vec(&json!({
        "action": "synchronize",
        "number": 77,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "Add more commits" }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&anthropic, 1, Duration::from_secs(8)).await,
        "pushed automation was not dispatched for synchronize action"
    );
}

/// An automation with trigger `github.pull_request:pushed` must NOT fire
/// when the event has `action=opened` — only `synchronize` maps to "pushed".
#[tokio::test]
async fn pushed_trigger_does_not_fire_on_opened_action() {
    let (_c, nats_port) = start_nats().await;
    let mock = MockServer::start_async().await;
    let mock_url = mock.base_url();

    let (_, js) = js_client(nats_port).await;
    create_streams(&js).await;

    let store = AutomationStore::open(&js).await.expect("open store");
    store
        .put(&make_automation(
            "auto-pushed-only",
            "default",
            "github.pull_request:pushed",
            "PUSHED_SHOULD_NOT_FIRE",
        ))
        .await
        .expect("put automation");

    // If the pushed automation fires → 500 (wrong dispatch).
    let bad_mock = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("PUSHED_SHOULD_NOT_FIRE");
            then.status(500)
                .body("pushed automation must not fire for opened action");
        })
        .await;

    // Fallback pr_review runs for "opened" when no automation matches.
    let fallback = mock
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("code reviewer");
            then.status(200)
                .header("content-type", "application/json")
                .body(end_turn_body());
        })
        .await;

    tokio::spawn(async move { run(runner_cfg(nats_port, mock_url, "default")).await.ok() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = serde_json::to_vec(&json!({
        "action": "opened",
        "number": 78,
        "repository": { "owner": { "login": "acme" }, "name": "api" },
        "pull_request": { "title": "New PR" }
    }))
    .unwrap();
    js.publish("github.pull_request", payload.into())
        .await
        .expect("publish");

    assert!(
        wait_for_hits(&fallback, 1, Duration::from_secs(8)).await,
        "fallback handler not called — pushed automation may have incorrectly fired"
    );
    assert_eq!(
        bad_mock.hits_async().await,
        0,
        "pushed automation fired for opened action"
    );
}
