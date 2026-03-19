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
//! - When `agent_memory_enabled = "off"`, memory fetch is skipped for automations
//!   (GitHub API not called) but the automation still runs.
//! - When `agent_memory_enabled = "on"`, memory is fetched from GitHub and
//!   included as the system prompt in the Anthropic request.
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-agent --test feature_flags_e2e

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use base64::Engine as _;
use httpmock::MockServer;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_agent::{AgentConfig, run};
use trogon_automations::{Automation, AutomationStore, Visibility};
use trogon_nats::{NatsAuth, NatsConfig};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, worker,
};
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
    stream::ensure_stream(&js, "trogon", &outbound_subject)
        .await
        .unwrap();

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
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let wjs = Arc::clone(&js);
    let wnats = nats.clone();
    let wstream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(wjs, wnats, vault, http_client, worker_name, &wstream)
            .await
            .ok();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;
    proxy_port
}

fn agent_config(nats_port: u16, proxy_url: String, split_url: Option<String>) -> AgentConfig {
    agent_config_with_memory(nats_port, proxy_url, split_url, None, None)
}

fn agent_config_with_memory(
    nats_port: u16,
    proxy_url: String,
    split_url: Option<String>,
    memory_owner: Option<String>,
    memory_repo: Option<String>,
) -> AgentConfig {
    AgentConfig {
        nats: NatsConfig::new(
            vec![format!("nats://127.0.0.1:{nats_port}")],
            NatsAuth::None,
        ),
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
        memory_owner,
        memory_repo,
        memory_path: None,
        mcp_servers: vec![],
        api_port: 0,
        tenant_id: "default".to_string(),
        split_evaluator_url: split_url,
        split_auth_token: None,
    }
}

/// When `split_auth_token` is configured it must be forwarded to the
/// evaluator as the `Authorization` header on every flag request.
/// The mock evaluator only returns `"on"` for the correct token — verifying
/// that the header is actually sent.
#[tokio::test]
async fn split_auth_token_is_forwarded_to_evaluator() {
    let (_nats_container, nats_port) = start_nats().await;

    // Evaluator stub that returns "on" only when the correct token is present.
    let evaluator_server = MockServer::start_async().await;
    let evaluator_mock = evaluator_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .header("authorization", "my-split-secret");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"treatment":"on"}"#);
        })
        .await;
    let evaluator_url = evaluator_server.base_url();

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));
    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();
    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "auth-token-worker",
    )
    .await;

    let mut cfg = agent_config(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        Some(evaluator_url),
    );
    cfg.split_auth_token = Some("my-split-secret".to_string());

    tokio::spawn(async move {
        run(cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    // Evaluator returns "on" only when the correct auth token is sent.
    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Anthropic must be called — evaluator returned 'on' for the correct auth token"
    );
    evaluator_mock.assert_async().await;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// When the evaluator returns HTTP 500 (as opposed to being unreachable),
/// `get_treatment_or_control` still returns `"control"` via the `Err` branch
/// → `is_enabled` returns `false` → handler is skipped (fail-closed).
///
/// This exercises a different reqwest code path than the dead-port test:
/// here the TCP connection succeeds but the HTTP response is an error status.
#[tokio::test]
async fn handler_skipped_when_evaluator_returns_500() {
    let (_nats_container, nats_port) = start_nats().await;

    // Evaluator stub that always returns 500.
    let evaluator_server = MockServer::start_async().await;
    evaluator_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment");
            then.status(500).body("internal error");
        })
        .await;
    let evaluator_url = evaluator_server.base_url();

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));
    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();
    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "eval-500-worker",
    )
    .await;

    let cfg = agent_config(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        Some(evaluator_url),
    );
    tokio::spawn(async move {
        run(cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must not be called when the evaluator returns 500 (fail-closed)"
    );
}

/// When the evaluator returns a custom variant (not `"on"` or `"off"`),
/// `is_enabled` must return `false` — only `"on"` enables a handler.
/// A misconfigured flag (`"enabled"`, `"true"`, `"1"`) must not accidentally
/// activate a handler.
#[tokio::test]
async fn handler_skipped_for_custom_variant_treatment() {
    let (_nats_container, nats_port) = start_nats().await;

    // MockEvaluator returns "enabled" — a plausible misconfiguration.
    let mock_evaluator = MockEvaluator::new().with_flag("agent_alert_handler_enabled", "enabled");
    let (evaluator_addr, _evaluator_handle) = mock_evaluator.serve().await;
    let evaluator_url = format!("http://{evaluator_addr}");

    let mock_server = MockServer::start_async().await;
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));
    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();
    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "custom-variant-worker",
    )
    .await;

    let cfg = agent_config(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        Some(evaluator_url),
    );
    tokio::spawn(async move {
        run(cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        anthropic_mock.hits_async().await,
        0,
        "Anthropic must not be called when evaluator returns custom variant 'enabled' (not 'on')"
    );
}

/// When `agent_alert_handler_enabled` is `"off"`, the runner silently acks the
/// JetStream message and Anthropic is never called.
#[tokio::test]
async fn alert_handler_skipped_when_flag_is_off() {
    let (_nats_container, nats_port) = start_nats().await;

    // Start MockEvaluator with the flag disabled.
    let mock_evaluator = MockEvaluator::new().with_flag("agent_alert_handler_enabled", "off");
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

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
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
    tokio::spawn(async move {
        run(cfg).await.ok();
    });
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
    let mock_evaluator = MockEvaluator::new().with_flag("agent_alert_handler_enabled", "on");
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

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
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
    tokio::spawn(async move {
        run(cfg).await.ok();
    });
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

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
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
    tokio::spawn(async move {
        run(cfg).await.ok();
    });
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
                .body(
                    r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Handled."}]}"#,
                );
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
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
    let cfg = agent_config(nats_port, format!("http://127.0.0.1:{proxy_port}"), None);
    tokio::spawn(async move {
        run(cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Anthropic must be called when no Split evaluator is configured (fail-open)"
    );
    anthropic_mock.assert_async().await;
}

/// Helper: register a `datadog.alert` automation in the NATS KV store so the
/// runner takes the `dispatch_automations` path (where `MemoryEnabled` is checked).
async fn register_datadog_automation(js: &jetstream::Context) {
    let store = AutomationStore::open(&Arc::new(js.clone())).await.unwrap();
    let auto = Automation {
        id: "mem-test-auto".to_string(),
        tenant_id: "default".to_string(),
        name: "Memory flag test automation".to_string(),
        trigger: "datadog.alert".to_string(),
        prompt: "Handle this Datadog alert.".to_string(),
        model: None,
        tools: vec![],
        memory_path: None,
        mcp_servers: vec![],
        enabled: true,
        visibility: Visibility::Private,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    };
    store.put(&auto).await.unwrap();
}

/// When `agent_memory_enabled = "off"`, the automation still runs (Anthropic is
/// called) but memory is NOT fetched (GitHub contents endpoint is never hit).
#[tokio::test]
async fn memory_fetch_skipped_when_flag_is_off() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_evaluator = MockEvaluator::new().with_flag("agent_memory_enabled", "off");
    let (evaluator_addr, _evaluator_handle) = mock_evaluator.serve().await;
    let evaluator_url = format!("http://{evaluator_addr}");

    let mock_server = MockServer::start_async().await;

    // GitHub memory endpoint — must NOT be hit.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/contents/.trogon/memory.md");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    serde_json::json!({
                        "content": base64::engine::general_purpose::STANDARD
                            .encode("This is the agent memory notes.")
                    })
                    .to_string(),
                );
        })
        .await;

    // Anthropic — must be called (automation runs regardless of memory flag).
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"handled"}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "mem-off-worker",
    )
    .await;

    register_datadog_automation(&js).await;

    let cfg = agent_config_with_memory(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        Some(evaluator_url),
        Some("test-org".to_string()),
        Some("test-repo".to_string()),
    );
    tokio::spawn(async move {
        run(cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    // Automation must complete (Anthropic called).
    let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "Anthropic must be called — automation runs even when memory flag is off"
    );

    // GitHub must NOT have been called.
    assert_eq!(
        github_mock.hits_async().await,
        0,
        "GitHub contents endpoint must not be hit when agent_memory_enabled = 'off'"
    );
}

/// When `agent_memory_enabled = "on"`, the automation fetches memory from GitHub
/// and includes it as the `system` prompt in the Anthropic request.
#[tokio::test]
async fn memory_content_included_when_flag_is_on() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_evaluator = MockEvaluator::new().with_flag("agent_memory_enabled", "on");
    let (evaluator_addr, _evaluator_handle) = mock_evaluator.serve().await;
    let evaluator_url = format!("http://{evaluator_addr}");

    let mock_server = MockServer::start_async().await;
    let memory_text = "Always respond in Spanish.";
    let memory_b64 = base64::engine::general_purpose::STANDARD.encode(memory_text);

    // GitHub memory endpoint — must be hit.
    let github_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/repos/test-org/test-repo/contents/.trogon/memory.md");
            then.status(200)
                .header("content-type", "application/json")
                .body(serde_json::json!({"content": memory_b64}).to_string());
        })
        .await;

    // Anthropic — must be called with memory as the `system` field.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .body_contains(memory_text);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"Hecho."}]}"#);
        })
        .await;

    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = Arc::new(jetstream::new(nats.clone()));

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_prod_test01").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let proxy_port = start_proxy_and_worker(
        &nats,
        Arc::clone(&js),
        mock_server.base_url(),
        Arc::clone(&vault),
        "mem-on-worker",
    )
    .await;

    register_datadog_automation(&js).await;

    let cfg = agent_config_with_memory(
        nats_port,
        format!("http://127.0.0.1:{proxy_port}"),
        Some(evaluator_url),
        Some("test-org".to_string()),
        Some("test-repo".to_string()),
    );
    tokio::spawn(async move {
        run(cfg).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    publish_datadog_alert(nats_port).await;

    // GitHub must have been called for memory.
    let hit = wait_for_hit(&github_mock, Duration::from_secs(20)).await;
    assert!(
        hit,
        "GitHub contents endpoint must be hit when agent_memory_enabled = 'on'"
    );

    // Anthropic must have been called with memory in the request body.
    anthropic_mock.assert_async().await;
}

// ── Generic publish helper ────────────────────────────────────────────────────

async fn publish_event(
    nats_port: u16,
    stream: &'static str,
    stream_subjects: &'static str,
    subject: &'static str,
    payload: serde_json::Value,
) {
    let nats = async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
        .await
        .unwrap();
    let js = jetstream::new(nats);
    js.get_or_create_stream(jetstream::stream::Config {
        name: stream.to_string(),
        subjects: vec![stream_subjects.to_string()],
        ..Default::default()
    })
    .await
    .unwrap();
    let bytes = serde_json::to_vec(&payload).unwrap();
    js.publish(subject, bytes.into())
        .await
        .unwrap()
        .await
        .unwrap();
}

// ── Per-handler "off" / "on" test pairs ──────────────────────────────────────

macro_rules! flag_tests {
    (
        off: $off_name:ident,
        on:  $on_name:ident,
        flag: $flag:literal,
        stream: $stream:literal,
        stream_subjects: $stream_subjects:literal,
        subject: $subject:literal,
        payload: $payload:expr $(,)?
    ) => {
        #[tokio::test]
        async fn $off_name() {
            let (_nats_container, nats_port) = start_nats().await;
            let mock_evaluator = MockEvaluator::new().with_flag($flag, "off");
            let (evaluator_addr, _evaluator_handle) = mock_evaluator.serve().await;
            let evaluator_url = format!("http://{evaluator_addr}");

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
                &nats, Arc::clone(&js), mock_server.base_url(),
                Arc::clone(&vault), concat!(stringify!($off_name), "-worker"),
            )
            .await;

            let cfg = agent_config(nats_port, format!("http://127.0.0.1:{proxy_port}"), Some(evaluator_url));
            tokio::spawn(async move { run(cfg).await.ok(); });
            tokio::time::sleep(Duration::from_millis(300)).await;

            publish_event(nats_port, $stream, $stream_subjects, $subject, $payload).await;

            tokio::time::sleep(Duration::from_secs(3)).await;
            assert_eq!(
                anthropic_mock.hits_async().await, 0,
                concat!("Anthropic must not be called when ", $flag, " = 'off'"),
            );
        }

        #[tokio::test]
        async fn $on_name() {
            let (_nats_container, nats_port) = start_nats().await;
            let mock_evaluator = MockEvaluator::new().with_flag($flag, "on");
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
                        .body(r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"handled"}]}"#);
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
                &nats, Arc::clone(&js), mock_server.base_url(),
                Arc::clone(&vault), concat!(stringify!($on_name), "-worker"),
            )
            .await;

            let cfg = agent_config(nats_port, format!("http://127.0.0.1:{proxy_port}"), Some(evaluator_url));
            tokio::spawn(async move { run(cfg).await.ok(); });
            tokio::time::sleep(Duration::from_millis(300)).await;

            publish_event(nats_port, $stream, $stream_subjects, $subject, $payload).await;

            let hit = wait_for_hit(&anthropic_mock, Duration::from_secs(20)).await;
            assert!(hit, concat!("Anthropic must be called when ", $flag, " = 'on'"));
            anthropic_mock.assert_async().await;
        }
    };
}

flag_tests! {
    off: pr_review_skipped_when_flag_is_off,
    on:  pr_review_runs_when_flag_is_on,
    flag: "agent_pr_review_enabled",
    stream: "GITHUB",
    stream_subjects: "github.>",
    subject: "github.pull_request",
    payload: serde_json::json!({
        "action": "opened",
        "number": 42,
        "repository": { "owner": { "login": "test-org" }, "name": "test-repo" }
    }),
}

flag_tests! {
    off: comment_handler_skipped_when_flag_is_off,
    on:  comment_handler_runs_when_flag_is_on,
    flag: "agent_comment_handler_enabled",
    stream: "GITHUB",
    stream_subjects: "github.>",
    subject: "github.issue_comment",
    payload: serde_json::json!({
        "action": "created",
        "repository": { "owner": { "login": "test-org" }, "name": "test-repo" },
        "issue": { "number": 42, "pull_request": {} },
        "comment": { "body": "What does this do?", "user": { "login": "alice" } }
    }),
}

flag_tests! {
    off: push_handler_skipped_when_flag_is_off,
    on:  push_handler_runs_when_flag_is_on,
    flag: "agent_push_handler_enabled",
    stream: "GITHUB",
    stream_subjects: "github.>",
    subject: "github.push",
    payload: serde_json::json!({
        "ref": "refs/heads/main",
        "deleted": false,
        "repository": { "owner": { "login": "test-org" }, "name": "test-repo" },
        "pusher": { "name": "alice" },
        "commits": [{ "message": "Fix bug" }],
        "head_commit": { "message": "Fix bug" }
    }),
}

flag_tests! {
    off: ci_handler_skipped_when_flag_is_off,
    on:  ci_handler_runs_when_flag_is_on,
    flag: "agent_ci_handler_enabled",
    stream: "GITHUB",
    stream_subjects: "github.>",
    subject: "github.check_run",
    payload: serde_json::json!({
        "action": "completed",
        "repository": { "owner": { "login": "test-org" }, "name": "test-repo" },
        "check_run": {
            "name": "CI",
            "conclusion": "failure",
            "details_url": "https://example.com/logs",
            "pull_requests": [{ "number": 42 }]
        }
    }),
}

flag_tests! {
    off: issue_triage_skipped_when_flag_is_off,
    on:  issue_triage_runs_when_flag_is_on,
    flag: "agent_issue_triage_enabled",
    stream: "LINEAR",
    stream_subjects: "linear.>",
    subject: "linear.Issue.create",
    payload: serde_json::json!({
        "action": "create",
        "type": "Issue",
        "data": { "id": "ISS-123", "title": "Fix authentication" }
    }),
}

flag_tests! {
    off: incidentio_handler_skipped_when_flag_is_off,
    on:  incidentio_handler_runs_when_flag_is_on,
    flag: "agent_incidentio_handler_enabled",
    stream: "INCIDENTIO",
    stream_subjects: "incidentio.>",
    subject: "incidentio.incident.created",
    payload: serde_json::json!({
        "event_type": "incident.created",
        "incident": {
            "id": "inc-123",
            "name": "API latency spike",
            "status": "triage",
            "severity": { "name": "P1" }
        }
    }),
}
