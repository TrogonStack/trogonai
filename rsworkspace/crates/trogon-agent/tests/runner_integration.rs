//! Integration tests for [`trogon_agent::runner`].
//!
//! Covers the full JetStream pull-consumer pipeline:
//!  - NATS connect error → `RunnerError::Nats`
//!  - PR event dispatched → Anthropic proxy called
//!  - Linear issue event dispatched → Anthropic proxy called
//!  - Irrelevant PR action (closed) → proxy NOT called
//!  - Irrelevant Linear event (update) → proxy NOT called
//!  - Custom stream names respected
//!
//! Requires Docker.  Run with:
//!   cargo test -p trogon-agent --test runner_integration

use std::time::Duration;

use async_nats::jetstream;
use bytes::Bytes;
use httpmock::MockServer;
use serde_json::json;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent::{AgentConfig, RunnerError};
use trogon_nats::{NatsAuth, NatsConfig};

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
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

fn make_config(nats_port: u16, proxy_url: &str) -> AgentConfig {
    AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
        proxy_url: proxy_url.to_string(),
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
        agent_id: None,
    }
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

fn end_turn_response() -> serde_json::Value {
    json!({
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": "Done." }]
    })
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

// ── Tests ─────────────────────────────────────────────────────────────────────

/// runner::run returns RunnerError::Nats when the credentials file does not exist.
///
/// `trogon_nats::connect` uses `.retry_on_initial_connect()`, so a bad host/port
/// never produces a ConnectError — instead the client "connects" but JetStream
/// ops time out.  The only path that yields `ConnectError::InvalidCredentials`
/// (→ RunnerError::Nats) is a missing credentials file, which is checked before
/// even opening a socket.
#[tokio::test]
async fn runner_nats_connect_error_missing_credentials() {
    let cfg = AgentConfig {
        nats: NatsConfig::new(
            vec!["nats://127.0.0.1:4222".to_string()],
            NatsAuth::Credentials("/nonexistent/path/creds.nk".into()),
        ),
        proxy_url: "http://localhost:9999".to_string(),
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
        agent_id: None,
    };

    let result = trogon_agent::run(cfg).await;
    assert!(
        matches!(result, Err(RunnerError::Nats(_))),
        "expected RunnerError::Nats for missing credentials file, got {result:?}"
    );
}

/// Full happy path: a github.pull_request event is consumed and the
/// Anthropic proxy endpoint is called exactly once.
#[tokio::test]
async fn runner_dispatches_github_pr_event_to_anthropic() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    // Give the runner time to bind consumers before we publish.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "action": "opened",
        "number": 42,
        "repository": {
            "owner": { "login": "acme" },
            "name": "backend"
        }
    });
    js.publish("github.pull_request", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — Anthropic proxy was never called for PR event"
    );
}

/// Full happy path: a linear.Issue.create event is consumed and the
/// Anthropic proxy endpoint is called exactly once.
#[tokio::test]
async fn runner_dispatches_linear_issue_event_to_anthropic() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "action": "create",
        "type": "Issue",
        "data": {
            "id": "ISS-42",
            "title": "Button is broken"
        }
    });
    js.publish("linear.Issue.create", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — Anthropic proxy was never called for Linear issue event"
    );
}

/// PR events with action "closed" must be silently skipped — proxy must NOT be called.
#[tokio::test]
async fn runner_skips_irrelevant_github_pr_action() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    // No mock registered — any call would return 404 and the test should never reach it.
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "action": "closed",
        "number": 7,
        "repository": {
            "owner": { "login": "acme" },
            "name": "backend"
        }
    });
    js.publish("github.pull_request", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    // Wait a short period and confirm the proxy was never called.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        mock.hits_async().await,
        0,
        "proxy must not be called for a 'closed' PR action"
    );
}

/// Linear events with action "update" must be silently skipped — proxy must NOT be called.
#[tokio::test]
async fn runner_skips_irrelevant_linear_event() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "action": "update",
        "type": "Issue",
        "data": { "id": "ISS-99", "title": "updated issue" }
    });
    js.publish("linear.Issue.update", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        mock.hits_async().await,
        0,
        "proxy must not be called for a non-create Linear event"
    );
}

// ── Handler error paths ───────────────────────────────────────────────────────

/// When a PR event payload is not valid JSON the handler returns
/// `Some(Err("JSON parse error: ..."))`.  The runner must log the error,
/// ack the message, and keep processing subsequent events.
#[tokio::test]
async fn runner_pr_handler_json_error_does_not_crash() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    // First: publish garbage bytes → handler JSON parse error, no proxy call.
    js.publish(
        "github.pull_request",
        Bytes::from_static(b"not json at all"),
    )
    .await
    .expect("publish failed");

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        mock.hits_async().await,
        0,
        "proxy must not be called for invalid JSON"
    );

    // Second: publish a valid event → runner is still alive and processes it.
    let valid = json!({
        "action": "opened",
        "number": 10,
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    });
    js.publish("github.pull_request", valid.to_string().into())
        .await
        .expect("publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "runner crashed after JSON parse error — second event was never processed"
    );
}

/// Same as above but for Linear issue events.
#[tokio::test]
async fn runner_issue_handler_json_error_does_not_crash() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    // Invalid bytes → handler JSON parse error.
    js.publish("linear.Issue.create", Bytes::from_static(b"{bad json"))
        .await
        .expect("publish failed");

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        mock.hits_async().await,
        0,
        "proxy must not be called for invalid JSON"
    );

    // Valid event → runner still alive.
    let valid = json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-1", "title": "Crash on login" }
    });
    js.publish("linear.Issue.create", valid.to_string().into())
        .await
        .expect("publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "runner crashed after JSON parse error — second Linear event was never processed"
    );
}

/// When the Anthropic proxy returns HTTP 500 the AgentLoop returns
/// `AgentError::Http` → handler returns `Some(Err(...))` → runner logs
/// `"PR review error"` and continues (does NOT crash).
#[tokio::test]
async fn runner_pr_handler_agent_error_does_not_crash() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;

    // First call: 500 → AgentError::Http → handler returns Some(Err(...)).
    let error_mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Review the pull request #1 ");
            then.status(500);
        })
        .await;

    // Second call (different PR number): 200 end_turn → success.
    let ok_mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("Review the pull request #2 ");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    // Event 1 → triggers agent error.
    let bad_event = json!({
        "action": "opened",
        "number": 1,
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    });
    js.publish("github.pull_request", bad_event.to_string().into())
        .await
        .expect("publish failed");

    // Wait for the error call to be made.
    assert!(
        wait_for_hit(&error_mock, Duration::from_secs(10)).await,
        "proxy was never called for the first (error) PR event"
    );

    // Event 2 → runner must still be alive and process it.
    let ok_event = json!({
        "action": "opened",
        "number": 2,
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    });
    js.publish("github.pull_request", ok_event.to_string().into())
        .await
        .expect("publish failed");

    assert!(
        wait_for_hit(&ok_mock, Duration::from_secs(10)).await,
        "runner crashed after AgentError::Http — second PR event was never processed"
    );
}

/// Same as above but for the Linear issue handler (exercises the
/// `Some(Err(e)) => error!("Issue triage error")` branch).
#[tokio::test]
async fn runner_issue_handler_agent_error_does_not_crash() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;

    let error_mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("ISS-ERR");
            then.status(500);
        })
        .await;

    let ok_mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("ISS-OK");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    let bad_event = json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-ERR", "title": "trigger error" }
    });
    js.publish("linear.Issue.create", bad_event.to_string().into())
        .await
        .expect("publish failed");

    assert!(
        wait_for_hit(&error_mock, Duration::from_secs(10)).await,
        "proxy was never called for the first (error) issue event"
    );

    let ok_event = json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-OK", "title": "trigger success" }
    });
    js.publish("linear.Issue.create", ok_event.to_string().into())
        .await
        .expect("publish failed");

    assert!(
        wait_for_hit(&ok_mock, Duration::from_secs(10)).await,
        "runner crashed after AgentError::Http — second issue event was never processed"
    );
}

/// A merged PR event (action=closed, merged=true) is routed to pr_merged handler.
#[tokio::test]
async fn runner_dispatches_pr_merged_event_to_anthropic() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.>"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "action": "closed",
        "number": 99,
        "pull_request": {
            "merged": true,
            "title": "Add feature X",
            "merged_by": { "login": "alice" }
        },
        "repository": { "owner": { "login": "acme" }, "name": "backend" }
    });
    js.publish("github.pull_request", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — Anthropic proxy was never called for PR merged event"
    );
}

/// A github.issue_comment event (action=created) is dispatched to Anthropic.
#[tokio::test]
async fn runner_dispatches_issue_comment_event_to_anthropic() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.>"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "action": "created",
        "issue": { "number": 7, "pull_request": {} },
        "comment": { "body": "Can you help?", "user": { "login": "bob" } },
        "repository": { "owner": { "login": "acme" }, "name": "backend" }
    });
    js.publish("github.issue_comment", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — Anthropic proxy was never called for issue_comment event"
    );
}

/// A github.push event to a branch is dispatched to Anthropic.
#[tokio::test]
async fn runner_dispatches_push_event_to_anthropic() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.>"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "ref": "refs/heads/feature-x",
        "deleted": false,
        "pusher": { "name": "carol" },
        "commits": [{ "id": "abc123" }],
        "head_commit": { "message": "add feature" },
        "repository": { "owner": { "login": "acme" }, "name": "backend" }
    });
    js.publish("github.push", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — Anthropic proxy was never called for push event"
    );
}

/// A github.check_run event with conclusion=failure is dispatched to Anthropic.
#[tokio::test]
async fn runner_dispatches_ci_failure_event_to_anthropic() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.>"]).await;
    create_stream(&js, "LINEAR", &["linear.>"]).await;
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "action": "completed",
        "check_run": {
            "name": "cargo test",
            "conclusion": "failure",
            "details_url": "https://ci.example.com/1",
            "pull_requests": [{ "number": 42 }]
        },
        "repository": { "owner": { "login": "acme" }, "name": "backend" }
    });
    js.publish("github.check_run", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — Anthropic proxy was never called for CI failure event"
    );
}

/// Custom stream names set in AgentConfig are respected by bind_consumer.
#[tokio::test]
async fn runner_respects_custom_stream_names() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    // Use non-default names — the default "GITHUB" / "LINEAR" streams do NOT exist.
    create_stream(&js, "MY_GH", &["github.pull_request"]).await;
    create_stream(&js, "MY_LIN", &["linear.>"]).await;
    // CRON_TICKS must always exist (runner binds it unconditionally).
    create_stream(&js, "CRON_TICKS", &["cron.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let mut cfg = make_config(nats_port, &proxy.base_url());
    cfg.github_stream_name = Some("MY_GH".to_string());
    cfg.linear_stream_name = Some("MY_LIN".to_string());

    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = json!({
        "action": "opened",
        "number": 1,
        "repository": { "owner": { "login": "org" }, "name": "repo" }
    });
    js.publish("github.pull_request", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — proxy was never called with custom stream names"
    );
}

// ── Stream auto-creation tests ────────────────────────────────────────────────
//
// The runner calls `ensure_stream` at startup for every required JetStream
// stream.  If a stream does not exist it is created automatically
// (`get_or_create_stream`), removing the hard startup-order dependency on
// trogon-github / trogon-linear / trogon-cron.
//
// These tests verify the replacement behaviour for the two deleted tests
// `runner_jetstream_error_when_github_stream_missing` and
// `runner_jetstream_error_when_linear_stream_missing`, which expected the
// runner to fail with `RunnerError::JetStream` when streams were absent.
// That error path was replaced by auto-creation; the runner now succeeds and
// processes events normally.

/// Poll until `stream_name` exists in JetStream (runner created it), or panic
/// after `timeout`.
async fn wait_for_stream(js: &jetstream::Context, stream_name: &str, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if js.get_stream(stream_name).await.is_ok() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "runner did not create JetStream stream '{}' within {:?}",
                stream_name, timeout
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// When no JetStream streams exist at startup the runner must create them and
/// successfully dispatch a GitHub pull-request event to the Anthropic proxy.
///
/// This is the direct replacement for the deleted
/// `runner_jetstream_error_when_github_stream_missing` test: instead of
/// expecting a `RunnerError::JetStream`, we verify the runner recovers by
/// creating the GITHUB stream itself and handling the event.
#[tokio::test]
async fn runner_creates_github_stream_when_missing_and_dispatches_event() {
    let (_container, nats_port) = start_nats().await;
    // Deliberately do NOT pre-create any streams — runner must create them.
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    // Wait until the runner has created the GITHUB stream (up to 10 s).
    wait_for_stream(&js, "GITHUB", Duration::from_secs(10)).await;

    let payload = json!({
        "action": "opened",
        "number": 7,
        "repository": {
            "owner": { "login": "acme" },
            "name": "backend"
        }
    });
    js.publish("github.pull_request", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — runner must dispatch GitHub PR event after auto-creating the stream"
    );
}

/// When no JetStream streams exist at startup the runner must create them and
/// successfully dispatch a Linear issue event to the Anthropic proxy.
///
/// This is the direct replacement for the deleted
/// `runner_jetstream_error_when_linear_stream_missing` test: instead of
/// expecting a `RunnerError::JetStream`, we verify the runner recovers by
/// creating the LINEAR stream itself and handling the event.
#[tokio::test]
async fn runner_creates_linear_stream_when_missing_and_dispatches_event() {
    let (_container, nats_port) = start_nats().await;
    // Deliberately do NOT pre-create any streams — runner must create them.
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);

    let proxy = MockServer::start_async().await;
    let mock = proxy
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(end_turn_response());
        })
        .await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    // Wait until the runner has created the LINEAR stream (up to 10 s).
    wait_for_stream(&js, "LINEAR", Duration::from_secs(10)).await;

    let payload = json!({
        "type": "Issue",
        "action": "create",
        "data": { "id": "abc-123", "title": "Bug report" }
    });
    js.publish("linear.Issue.create", payload.to_string().into())
        .await
        .expect("JetStream publish failed");

    assert!(
        wait_for_hit(&mock, Duration::from_secs(10)).await,
        "timed out — runner must dispatch Linear issue event after auto-creating the stream"
    );
}
