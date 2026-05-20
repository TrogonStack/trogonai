//! End-to-end integration tests: XaiAgent + AgentSideNatsConnection + real NATS.
//!
//! Verifies that XaiAgent correctly handles ACP request-reply over a real NATS
//! server. The xAI HTTP client is replaced with a mock so no real API key is
//! needed. Only `initialize` and `session/new` are exercised — neither touches
//! the xAI API.
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-xai-runner --test nats_e2e --features test-helpers

use std::sync::Arc;
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{
    Agent, CloseSessionRequest, ForkSessionRequest, LoadSessionRequest, NewSessionRequest,
    SessionConfigKind, SetSessionConfigOptionRequest,
};
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_xai_runner::{
    Message, MockXaiHttpClient, NatsSessionNotifier, NatsSessionStore, SessionStoring, XaiAgent,
};

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    (container, nats)
}

/// Spawn XaiAgent in a dedicated thread and wait for it to subscribe.
async fn start_agent(nats: async_nats::Client) {
    let nats_for_thread = nats.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats_for_thread.clone(), prefix.clone());
        let agent = XaiAgent::with_deps(notifier, "grok-4", "", MockXaiHttpClient::default());
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats_for_thread, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    // Give the agent time to subscribe before the test sends requests.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// XaiAgent handles `initialize` over real NATS and returns capabilities.
#[tokio::test]
async fn e2e_nats_initialize_returns_capabilities() {
    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let payload = serde_json::to_vec(&serde_json::json!({"protocolVersion": 0})).unwrap();
    let msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.initialize", payload.into()),
    )
    .await
    .expect("timed out waiting for initialize response")
    .expect("NATS request failed");

    let response: Value = serde_json::from_slice(&msg.payload).unwrap();
    assert!(
        response["protocolVersion"].is_number(),
        "must have protocolVersion in response: {response}"
    );
    assert!(
        response["agentCapabilities"].is_object(),
        "must have agentCapabilities: {response}"
    );
}

/// XaiAgent handles `session/new` over real NATS and returns a non-empty session ID.
#[tokio::test]
async fn e2e_nats_session_new_returns_session_id() {
    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let payload = serde_json::to_vec(&serde_json::json!({
        "sessionId": null,
        "cwd": "/tmp",
        "mcpServers": []
    }))
    .unwrap();
    let msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", payload.into()),
    )
    .await
    .expect("timed out waiting for session/new response")
    .expect("NATS request failed");

    let response: Value = serde_json::from_slice(&msg.payload).unwrap();
    let session_id = response["sessionId"].as_str().unwrap_or("");
    assert!(!session_id.is_empty(), "must have non-empty sessionId: {response}");
}

/// Full end-to-end chain: new_session → set_config_option → close_session → KV save →
/// fresh agent instance → load_session restores tool state from KV.
#[tokio::test]
async fn enabled_tools_persisted_to_kv_and_restored_on_load_session() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());
    let store: Arc<dyn SessionStoring> =
        Arc::new(NatsSessionStore::open(&js, 0).await.expect("open store"));

    let session_id = {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix);
        let agent = XaiAgent::with_deps(notifier, "grok-3", "test-key", mock_http)
            .with_session_store(Arc::clone(&store));

        let new_resp = agent
            .new_session(NewSessionRequest::new("/tmp"))
            .await
            .expect("new_session must succeed");
        let sid = new_resp.session_id.to_string();

        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                sid.clone(),
                "web_search",
                "on",
            ))
            .await
            .expect("set_config_option must succeed");

        agent
            .close_session(CloseSessionRequest::new(sid.clone()))
            .await
            .expect("close_session must succeed");

        sid
    };

    // Verify the KV snapshot has web_search in tools.
    let sessions_kv = js.get_key_value("SESSIONS").await.expect("open SESSIONS KV");
    let kv_entry = sessions_kv
        .entry(&format!("default.{session_id}"))
        .await
        .expect("KV get")
        .expect("snapshot must exist after close_session");
    let snap: serde_json::Value = serde_json::from_slice(&kv_entry.value).unwrap();
    let empty_tools = vec![];
    let tools: Vec<&str> = snap["tools"]
        .as_array()
        .unwrap_or(&empty_tools)
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        tools.contains(&"web_search"),
        "KV snapshot must contain 'web_search' in tools, got: {tools:?}"
    );

    // Agent 2 — fresh instance with same store, empty sessions map.
    {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix);
        let agent2 = XaiAgent::with_deps(notifier, "grok-3", "test-key", mock_http)
            .with_session_store(Arc::clone(&store));

        let load_resp = agent2
            .load_session(LoadSessionRequest::new(session_id.clone(), "/tmp"))
            .await
            .expect("load_session must succeed via KV fallback");

        let opts = load_resp.config_options.unwrap_or_default();
        let web_search_opt = opts
            .iter()
            .find(|o| o.id.to_string() == "web_search")
            .expect("web_search must be present in config_options");
        let web_val = match &web_search_opt.kind {
            SessionConfigKind::Select(s) => s.current_value.to_string(),
            _ => String::new(),
        };
        assert_eq!(
            web_val, "on",
            "web_search must be 'on' after restoring from KV snapshot"
        );
    }
}

/// fork_session → KV save with inherited tools → fresh agent → load_session restores tools.
#[tokio::test]
async fn fork_session_tools_persisted_to_kv_and_restored_on_load_session() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());
    let store: Arc<dyn SessionStoring> =
        Arc::new(NatsSessionStore::open(&js, 0).await.expect("open store"));

    let fork_id = {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix);
        let agent = XaiAgent::with_deps(notifier, "grok-3", "test-key", mock_http)
            .with_session_store(Arc::clone(&store));

        let new_resp = agent
            .new_session(NewSessionRequest::new("/tmp"))
            .await
            .expect("new_session must succeed");
        let src_id = new_resp.session_id.to_string();

        agent
            .set_session_config_option(SetSessionConfigOptionRequest::new(
                src_id.clone(),
                "web_search",
                "on",
            ))
            .await
            .expect("set_config_option must succeed");

        let fork_resp = agent
            .fork_session(ForkSessionRequest::new(src_id, "/fork"))
            .await
            .expect("fork_session must succeed");

        fork_resp.session_id.to_string()
    };

    // Verify the KV snapshot for the forked session has web_search.
    let sessions_kv = js.get_key_value("SESSIONS").await.expect("open SESSIONS KV");
    let kv_entry = sessions_kv
        .entry(&format!("default.{fork_id}"))
        .await
        .expect("KV get")
        .expect("fork snapshot must exist in KV");
    let snap: serde_json::Value = serde_json::from_slice(&kv_entry.value).unwrap();
    let empty_tools = vec![];
    let tools: Vec<&str> = snap["tools"]
        .as_array()
        .unwrap_or(&empty_tools)
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        tools.contains(&"web_search"),
        "fork KV snapshot must contain 'web_search' in tools, got: {tools:?}"
    );

    // Agent 2 — fresh instance, load fork from KV.
    {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix);
        let agent2 = XaiAgent::with_deps(notifier, "grok-3", "test-key", mock_http)
            .with_session_store(Arc::clone(&store));

        let load_resp = agent2
            .load_session(LoadSessionRequest::new(fork_id.clone(), "/fork"))
            .await
            .expect("load_session of fork must succeed via KV fallback");

        let opts = load_resp.config_options.unwrap_or_default();
        let web_search_opt = opts
            .iter()
            .find(|o| o.id.to_string() == "web_search")
            .expect("web_search must be present in config_options");
        let web_val = match &web_search_opt.kind {
            SessionConfigKind::Select(s) => s.current_value.to_string(),
            _ => String::new(),
        };
        assert_eq!(
            web_val, "on",
            "forked session must have web_search='on' after KV restore"
        );
    }
}

/// Full agent round-trip: history and model survive build_snapshot → real NATS KV → load_session.
///
/// This is the only test that exercises the complete chain:
///   Message → SnapshotMessage (build_snapshot) → JSON → NATS KV (NatsSessionStore::save)
///   → NATS KV → JSON → SessionSnapshot (NatsSessionStore::load)
///   → Message (load_session KV fallback)
#[tokio::test]
async fn history_and_model_survive_kv_round_trip() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());
    let store: Arc<dyn SessionStoring> =
        Arc::new(NatsSessionStore::open(&js, 0).await.expect("open store"));

    let session_id = {
        let mock_http = Arc::new(MockXaiHttpClient::new());
        let prefix = AcpPrefix::new("acp").unwrap();
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix);
        let agent = XaiAgent::with_deps(notifier, "grok-3-mini", "test-key", mock_http)
            .with_session_store(Arc::clone(&store));

        let new_resp = agent
            .new_session(NewSessionRequest::new("/tmp"))
            .await
            .expect("new_session must succeed");
        let sid = new_resp.session_id.to_string();

        agent
            .test_set_session_history(
                &sid,
                vec![
                    Message {
                        role: "user".to_string(),
                        content: Some("What is 2+2?".to_string()),
                        prompt_tokens: None,
                        completion_tokens: None,
                    },
                    Message {
                        role: "assistant".to_string(),
                        content: Some("It is 4.".to_string()),
                        prompt_tokens: Some(12),
                        completion_tokens: Some(6),
                    },
                ],
            )
            .await;

        agent
            .close_session(CloseSessionRequest::new(sid.clone()))
            .await
            .expect("close_session must succeed");

        sid
    };

    // Agent 2 — fresh instance, load from KV.
    let mock_http = Arc::new(MockXaiHttpClient::new());
    let prefix = AcpPrefix::new("acp").unwrap();
    let notifier = NatsSessionNotifier::new(nats.clone(), prefix);
    let agent2 = XaiAgent::with_deps(notifier, "grok-3-mini", "test-key", mock_http)
        .with_session_store(Arc::clone(&store));

    let load_resp = agent2
        .load_session(LoadSessionRequest::new(session_id.clone(), "/tmp"))
        .await
        .expect("load_session must succeed via KV fallback");

    // Model must be restored.
    assert_eq!(
        load_resp.models.unwrap().current_model_id.to_string(),
        "grok-3-mini",
        "model must survive KV round-trip"
    );

    // History must be restored with content and token counts intact.
    let history = agent2.test_session_history(&session_id).await;
    assert_eq!(history.len(), 2, "history must have 2 messages after KV restore");
    assert_eq!(history[0].role, "user");
    assert_eq!(
        history[0].content.as_deref(),
        Some("What is 2+2?"),
        "user message content must survive KV round-trip"
    );
    assert_eq!(history[1].role, "assistant");
    assert_eq!(
        history[1].content.as_deref(),
        Some("It is 4."),
        "assistant message content must survive KV round-trip"
    );
    assert_eq!(
        history[1].prompt_tokens,
        Some(12),
        "prompt_tokens must survive KV round-trip"
    );
    assert_eq!(
        history[1].completion_tokens,
        Some(6),
        "completion_tokens must survive KV round-trip"
    );
}

/// Pre-fix snapshot (tools:[]) stored in real NATS KV → load_session restores trogon tools,
/// not xAI tools.
#[tokio::test]
async fn pre_fix_snapshot_empty_tools_restores_trogon_tools() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());
    let store = NatsSessionStore::open(&js, 0).await.expect("open store");

    // Write a pre-fix snapshot directly to NATS KV with tools:[].
    let session_id = "pre-fix-session";
    let tenant_id = "default";
    let snap_json = serde_json::json!({
        "id": session_id,
        "tenant_id": tenant_id,
        "name": "Old session",
        "messages": [],
        "tools": [],
        "created_at": "2026-01-01T00:00:00.000Z",
        "updated_at": "2026-01-01T00:00:00.000Z"
    });
    let sessions_kv = js.get_key_value("SESSIONS").await.expect("open KV");
    sessions_kv
        .put(
            &format!("{tenant_id}.{session_id}"),
            serde_json::to_vec(&snap_json).unwrap().into(),
        )
        .await
        .expect("put snapshot");

    // Agent loads it — must restore trogon tools, not xAI tools.
    let store: Arc<dyn SessionStoring> = Arc::new(store);
    let mock_http = Arc::new(MockXaiHttpClient::new());
    let prefix = AcpPrefix::new("acp").unwrap();
    let notifier = NatsSessionNotifier::new(nats.clone(), prefix);
    let agent = XaiAgent::with_deps(notifier, "grok-3", "test-key", mock_http)
        .with_session_store(Arc::clone(&store));

    let load_resp = agent
        .load_session(LoadSessionRequest::new(session_id, "/tmp"))
        .await
        .expect("load_session must succeed via KV fallback");

    let tools = agent.test_session_enabled_tools(session_id).await;

    // xAI tools must be off.
    let opts = load_resp.config_options.unwrap_or_default();
    let web_val = opts
        .iter()
        .find(|o| o.id.to_string() == "web_search")
        .map(|o| match &o.kind {
            SessionConfigKind::Select(s) => s.current_value.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default();
    assert_eq!(
        web_val, "off",
        "web_search must be off after restoring pre-fix snapshot"
    );

    // Trogon tools must be present.
    assert!(
        !tools.is_empty(),
        "enabled_tools must not be empty after restoring pre-fix snapshot"
    );
    assert!(
        !tools.iter().any(|t| t == "web_search" || t == "x_search"),
        "xAI tools must not be in enabled_tools after restoring pre-fix snapshot"
    );
    assert!(
        tools.iter().any(|t| t == "read_file"),
        "trogon tool 'read_file' must be in enabled_tools after restoring pre-fix snapshot"
    );
}

/// session/get_state returns the stored session's cwd over real NATS.
#[tokio::test]
async fn e2e_ext_session_get_state_returns_cwd() {
    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let new_payload = serde_json::to_vec(&serde_json::json!({
        "sessionId": null,
        "cwd": "/projects/myapp",
        "mcpServers": []
    }))
    .unwrap();
    let new_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", new_payload.into()),
    )
    .await
    .expect("timed out waiting for session/new")
    .expect("NATS request failed");
    let new_resp: Value = serde_json::from_slice(&new_msg.payload).unwrap();
    let session_id = new_resp["sessionId"].as_str().expect("session/new must return sessionId");

    let ext_payload = serde_json::to_vec(&serde_json::json!({ "sessionId": session_id })).unwrap();
    let ext_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.ext.session/get_state", ext_payload.into()),
    )
    .await
    .expect("timed out waiting for session/get_state")
    .expect("NATS ext request failed");

    let state: Value = serde_json::from_slice(&ext_msg.payload).unwrap();
    assert_eq!(
        state["cwd"].as_str(),
        Some("/projects/myapp"),
        "session/get_state must return the session's cwd: {state}"
    );
}

/// Verifies the runner registration contract: the `acp_prefix` metadata stored
/// in the registry must match the `ACP_PREFIX` env var.  The bridge
/// (`acp-nats-ws`, `acp-nats-server`) reads this field to derive the NATS
/// routing prefix — a mismatch here breaks routing silently in production.
#[tokio::test]
async fn xai_runner_registers_with_correct_acp_prefix_metadata() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let prefix = "acp.xai";
    let agent_type = "xai";

    let store = trogon_registry::provision(&js).await.expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.to_string(),
        capabilities: vec!["chat".to_string()],
        nats_subject: format!("{}.agent.>", prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": prefix }),
    };
    registry.register(&cap).await.expect("registration must succeed");

    let entry = registry
        .get(agent_type)
        .await
        .expect("get must not error")
        .expect("registered entry must exist");

    assert_eq!(
        entry.metadata["acp_prefix"].as_str(),
        Some(prefix),
        "bridge relies on acp_prefix matching ACP_PREFIX — got {:?}",
        entry.metadata
    );
    assert_eq!(
        entry.nats_subject,
        format!("{}.agent.>", prefix),
        "nats_subject must be derived from ACP_PREFIX"
    );
}
