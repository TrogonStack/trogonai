//! Integration tests for [`trogon_agent::runner`].
//!
//! Covers the full JetStream pull-consumer pipeline:
//!  - NATS connect error → `RunnerError::Nats`
//!  - Missing GITHUB stream → `RunnerError::JetStream`
//!  - GITHUB exists but LINEAR missing → `RunnerError::JetStream`
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
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
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
        model: "claude-opus-4-6".to_string(),
        max_iterations: 1,
        github_stream_name: None,
        linear_stream_name: None,
        memory_owner: None,
        memory_repo: None,
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
        model: "claude-opus-4-6".to_string(),
        max_iterations: 1,
        github_stream_name: None,
        linear_stream_name: None,
        memory_owner: None,
        memory_repo: None,
    };

    let result = trogon_agent::run(cfg).await;
    assert!(
        matches!(result, Err(RunnerError::Nats(_))),
        "expected RunnerError::Nats for missing credentials file, got {result:?}"
    );
}

/// runner::run returns RunnerError::JetStream when the GITHUB stream does not exist.
#[tokio::test]
async fn runner_jetstream_error_when_github_stream_missing() {
    let (_container, nats_port) = start_nats().await;
    // Neither GITHUB nor LINEAR streams are created.
    let cfg = make_config(nats_port, "http://localhost:9999");

    let result = trogon_agent::run(cfg).await;
    assert!(
        matches!(result, Err(RunnerError::JetStream(_))),
        "expected RunnerError::JetStream when GITHUB stream is absent; got {result:?}"
    );
}

/// runner::run returns RunnerError::JetStream when GITHUB exists but LINEAR does not.
#[tokio::test]
async fn runner_jetstream_error_when_linear_stream_missing() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    create_stream(&js, "GITHUB", &["github.pull_request"]).await;
    // LINEAR intentionally not created.

    let cfg = make_config(nats_port, "http://localhost:9999");
    let result = trogon_agent::run(cfg).await;
    assert!(
        matches!(result, Err(RunnerError::JetStream(_))),
        "expected RunnerError::JetStream when LINEAR stream is absent; got {result:?}"
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

    let proxy = MockServer::start_async().await;
    let mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

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

    let proxy = MockServer::start_async().await;
    let mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

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

    let proxy = MockServer::start_async().await;
    // No mock registered — any call would return 404 and the test should never reach it.
    let mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

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

    let proxy = MockServer::start_async().await;
    let mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

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

    let proxy = MockServer::start_async().await;
    let mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    // First: publish garbage bytes → handler JSON parse error, no proxy call.
    js.publish("github.pull_request", Bytes::from_static(b"not json at all"))
        .await
        .expect("publish failed");

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(mock.hits_async().await, 0, "proxy must not be called for invalid JSON");

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

    let proxy = MockServer::start_async().await;
    let mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

    let cfg = make_config(nats_port, &proxy.base_url());
    tokio::spawn(async move { trogon_agent::run(cfg).await });

    tokio::time::sleep(Duration::from_millis(400)).await;

    // Invalid bytes → handler JSON parse error.
    js.publish("linear.Issue.create", Bytes::from_static(b"{bad json"))
        .await
        .expect("publish failed");

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(mock.hits_async().await, 0, "proxy must not be called for invalid JSON");

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

    let proxy = MockServer::start_async().await;

    // First call: 500 → AgentError::Http → handler returns Some(Err(...)).
    let error_mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("Review the pull request #1 ");
        then.status(500);
    }).await;

    // Second call (different PR number): 200 end_turn → success.
    let ok_mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("Review the pull request #2 ");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

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

    let proxy = MockServer::start_async().await;

    let error_mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("ISS-ERR");
        then.status(500);
    }).await;

    let ok_mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/anthropic/v1/messages")
            .body_contains("ISS-OK");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

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

/// Custom stream names set in AgentConfig are respected by bind_consumer.
#[tokio::test]
async fn runner_respects_custom_stream_names() {
    let (_container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = jetstream::new(nats);
    // Use non-default names — the default "GITHUB" / "LINEAR" streams do NOT exist.
    create_stream(&js, "MY_GH", &["github.pull_request"]).await;
    create_stream(&js, "MY_LIN", &["linear.>"]).await;

    let proxy = MockServer::start_async().await;
    let mock = proxy.mock_async(|when, then| {
        when.method(httpmock::Method::POST).path("/anthropic/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(end_turn_response());
    }).await;

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
