//! E2e tests: feature flags gate agent handlers via Split.io MockEvaluator.
//!
//! These tests verify the full flag → runner → handler path:
//!
//! - When `agent_alert_handler_enabled = "off"`, a Datadog alert message
//!   is silently acked and Anthropic is **never** called.
//! - When `agent_alert_handler_enabled = "on"`, the handler runs and
//!   Anthropic **is** called.
//! - When the evaluator is configured but unreachable, `get_treatment_or_control`
//!   returns `"control"` → `is_enabled` returns `false` → handler is skipped
//!   (fail-closed for a misconfigured evaluator).
//! - When `SPLIT_EVALUATOR_URL` is not set at all (`split_evaluator_url: None`),
//!   all flags default to `true` (fail-open) → handler runs.
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test feature_flags_e2e

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use httpmock::MockServer;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent::{AgentConfig, run};
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_secret_proxy::{proxy::{ProxyState, router}, stream, subjects, worker};
use trogon_splitio::mock::MockEvaluator;
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

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

/// Publish a Datadog alert message directly to JetStream (bypasses the HTTP
/// webhook server — faster and simpler for flag-gate testing).
async fn publish_datadog_alert(nats_port: u16) {
    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = jetstream::new(nats);

    js.get_or_create_stream(jetstream::stream::Config {
        name: "DATADOG".to_string(),
        subjects: vec!["datadog.>".to_string()],
        ..Default::default()
    })
    .await
    .unwrap();

    let payload = serde_json::to_vec(&serde_json::json!({
        "alert_transition": "Triggered",
        "alert_id": "flag-test-001",
        "alert_title": "Feature flag gate test alert",
        "host": "web-01"
    }))
    .unwrap();

    js.publish("datadog.alert", payload.into())
        .await
        .unwrap()
        .await
        .unwrap();
}

async fn start_proxy_and_worker(
    nats: &async_nats::Client,
    js: Arc<jetstream::Context>,
    mock_base_url: String,
    vault: Arc<MemoryVault>,
    worker_name: &'static str,
) -> u16 {
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&js, "trogon", &outbound_subject).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: Arc::clone(&js),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_base_url),
    };
    tokio::spawn(async move { axum::serve(listener, router(proxy_state)).await.ok(); });

    let http_client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap();
    let wjs = Arc::clone(&js);
    let wnats = nats.clone();
    let wstream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(wjs, wnats, vault, http_client, worker_name, &wstream).await.ok();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;
    proxy_port
}

fn agent_config(nats_port: u16, proxy_url: String, split_url: Option<String>) -> AgentConfig {
    AgentConfig {
        nats: NatsConfig::new(vec![format!("nats://127.0.0.1:{nats_port}")], NatsAuth::None),
        proxy_url,
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
        incidentio_stream_name: None,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        split_evaluator_url: split_url,
        split_auth_token: None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// When `agent_alert_handler_enabled` is `"off"`, the runner silently acks the
/// JetStream message and Anthropic is never called.
#[tokio::test]
async fn alert_handler_skipped_when_flag_is_off() {
    let (_nats_container, nats_port) = start_nats().await;

    // Start MockEvaluator with the flag disabled.
    let mock_evaluator = MockEvaluator::new()
        .with_flag("agent_alert_handler_enabled", "off");
    let (evaluator_addr, _evaluator_handle) = mock_evaluator.serve().await;
    let evaluator_url = format!("http://{evaluator_addr}");

    // Mock Anthropic — we'll assert it was NOT called.
    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(&ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(), "sk-ant-realkey")
        .await
        .unwrap();

    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "flag-off-worker",
    )
    .await;

    let cfg = agent_config(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        Some(evaluator_url),
    );
    tokio::spawn(async move { run(cfg).await.ok(); });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Publish a Datadog alert — handler should be gated off.
    publish_datadog_alert(nats_port).await;

    // Wait a generous window and assert Anthropic was never called.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must not be called when agent_alert_handler_enabled = 'off'"
    );
}

/// When `agent_alert_handler_enabled` is `"on"`, the handler runs end-to-end
/// and Anthropic receives the real API key (via proxy + worker).
#[tokio::test]
async fn alert_handler_runs_when_flag_is_on() {
    let (_nats_container, nats_port) = start_nats().await;

    // Start MockEvaluator with the flag enabled.
    let mock_evaluator = MockEvaluator::new()
        .with_flag("agent_alert_handler_enabled", "on");
    let (evaluator_addr, _evaluator_handle) = mock_evaluator.serve().await;
    let evaluator_url = format!("http://{evaluator_addr}");

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Alert handled."}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(&ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(), "sk-ant-realkey")
        .await
        .unwrap();

    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "flag-on-worker",
    )
    .await;

    let cfg = agent_config(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        Some(evaluator_url),
    );
    tokio::spawn(async move { run(cfg).await.ok(); });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Anthropic must be called when agent_alert_handler_enabled = 'on'"
    );
    anthropic_mock.assert_async().await;
}

/// When the evaluator is configured but unreachable (dead port), every flag
/// evaluation returns `"control"` via `get_treatment_or_control`.  Since
/// `"control" != "on"`, `is_enabled` returns `false` and the handler is skipped.
///
/// This is fail-closed for a misconfigured evaluator — distinct from the
/// fail-open behaviour when `SPLIT_EVALUATOR_URL` is not set at all.
#[tokio::test]
async fn handler_skipped_when_evaluator_is_unreachable() {
    let (_nats_container, nats_port) = start_nats().await;

    // Point at a port where nothing listens — evaluator is "down".
    let dead_evaluator_url = "http://127.0.0.1:1".to_string();

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(&ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(), "sk-ant-realkey")
        .await
        .unwrap();

    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "flag-dead-worker",
    )
    .await;

    let cfg = agent_config(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        Some(dead_evaluator_url),
    );
    tokio::spawn(async move { run(cfg).await.ok(); });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    // Handler skipped — Anthropic must not be called.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must not be called when the evaluator is unreachable \
         (control treatment → is_enabled = false)"
    );
}

/// When `split_evaluator_url` is `None` (not configured), all flags default
/// to `true` (fail-open) regardless of what any evaluator would return.
/// The handler runs and Anthropic is called normally.
#[tokio::test]
async fn fail_open_when_split_evaluator_not_configured() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Handled."}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}")).await.unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(&ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(), "sk-ant-realkey")
        .await
        .unwrap();

    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "fail-open-worker",
    )
    .await;

    // No split_evaluator_url → all flags default to true.
    let cfg = agent_config(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        None,
    );
    tokio::spawn(async move { run(cfg).await.ok(); });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Anthropic must be called when no Split evaluator is configured (fail-open)"
    );
    anthropic_mock.assert_async().await;
}
