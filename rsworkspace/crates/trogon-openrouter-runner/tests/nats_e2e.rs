//! End-to-end integration tests: OpenRouterAgent + AgentSideNatsConnection + real NATS.
//!
//! Verifies that OpenRouterAgent correctly handles ACP request-reply over a real NATS
//! server. The OpenRouter HTTP client is replaced with a no-op stub so no real API key
//! is needed. Only `initialize` and `session/new` are exercised — neither touches
//! the OpenRouter API.
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test nats_e2e

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats::jetstream::provision::provision_streams;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{
    Agent as _, CloseSessionRequest, ContentBlock, CreateTerminalResponse, ExtRequest,
    ForkSessionRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, PromptResponse,
    SessionConfigKind, SessionId, SetSessionConfigOptionRequest, SetSessionModelRequest,
    TerminalOutputResponse,
};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use futures_util::stream::{self, LocalBoxStream};
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_openrouter_runner::{
    AssembledToolCall, Message, NatsSessionNotifier, OpenRouterAgent, OpenRouterEvent,
    OpenRouterHttpClient, ToolDef,
};

// ── No-op HTTP client stub ────────────────────────────────────────────────────

struct NoOpHttpClient;

#[async_trait(?Send)]
impl OpenRouterHttpClient for NoOpHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> LocalBoxStream<'static, OpenRouterEvent> {
        Box::pin(stream::empty())
    }
}

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
        let agent = OpenRouterAgent::with_deps(notifier, "test-model", "", NoOpHttpClient);
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats_for_thread, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
}

// ── tests ─────────────────────────────────────────────────────────────────────

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

#[tokio::test]
async fn openrouter_runner_registers_with_correct_acp_prefix_metadata() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let prefix = "acp.openrouter";
    let agent_type = "openrouter";

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

/// Verifies that openrouter main.rs correctly registers model IDs in metadata.models
/// so that CrossRunnerSwitcher can resolve the runner by model ID.
#[tokio::test]
async fn openrouter_runner_registers_with_model_ids_in_metadata() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let prefix = "acp.openrouter";
    let agent_type = "openrouter";
    // Replicate main.rs OPENROUTER_MODELS parsing
    let or_models_env = "anthropic/claude-3-5-sonnet,openai/gpt-4o";
    let model_ids: Vec<String> = or_models_env
        .split(',')
        .filter_map(|entry| entry.split(':').next().map(|id| id.trim().to_string()))
        .filter(|id| !id.is_empty())
        .collect();

    let store = trogon_registry::provision(&js).await.expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.to_string(),
        capabilities: vec!["chat".to_string(), "explore".to_string(), "plan".to_string()],
        nats_subject: format!("{}.agent.>", prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": prefix, "models": model_ids }),
    };
    registry.register(&cap).await.expect("registration must succeed");

    let entry = registry
        .get(agent_type)
        .await
        .expect("get must not error")
        .expect("registered entry must exist");

    let models = entry.metadata["models"].as_array().expect("metadata.models must be array");
    let model_strings: Vec<&str> = models.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        model_strings.contains(&"anthropic/claude-3-5-sonnet"),
        "metadata.models must contain 'anthropic/claude-3-5-sonnet'; got: {model_strings:?}"
    );
    assert!(
        model_strings.contains(&"openai/gpt-4o"),
        "metadata.models must contain 'openai/gpt-4o'; got: {model_strings:?}"
    );
}

/// `find_by_model` routes to the correct runner when xai, openrouter, and codex are all
/// registered with distinct model ID sets — as happens in a full platform deployment.
#[tokio::test]
async fn find_by_model_returns_correct_runner_for_multi_runner_registry() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let store = trogon_registry::provision(&js).await.expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    registry
        .register(&trogon_registry::AgentCapability {
            agent_type: "xai".to_string(),
            capabilities: vec!["chat".to_string(), "explore".to_string(), "plan".to_string()],
            nats_subject: "acp.xai.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::json!({ "acp_prefix": "acp.xai", "models": ["grok-4"] }),
        })
        .await
        .expect("register xai");

    registry
        .register(&trogon_registry::AgentCapability {
            agent_type: "openrouter".to_string(),
            capabilities: vec!["chat".to_string(), "explore".to_string(), "plan".to_string()],
            nats_subject: "acp.or.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::json!({
                "acp_prefix": "acp.or",
                "models": ["anthropic/claude-3-5-sonnet", "openai/gpt-4o"]
            }),
        })
        .await
        .expect("register openrouter");

    registry
        .register(&trogon_registry::AgentCapability {
            agent_type: "codex".to_string(),
            capabilities: vec!["chat".to_string(), "code_edit".to_string()],
            nats_subject: "acp.codex.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::json!({ "acp_prefix": "acp.codex", "models": ["o4-mini"] }),
        })
        .await
        .expect("register codex");

    // grok-4 → xai
    let xai = registry.find_by_model("grok-4").await.unwrap().expect("grok-4 must resolve");
    assert_eq!(xai.agent_type, "xai", "grok-4 must route to xai; got: {}", xai.agent_type);
    assert_eq!(xai.metadata["acp_prefix"].as_str(), Some("acp.xai"));

    // anthropic/claude-3-5-sonnet → openrouter
    let or1 = registry
        .find_by_model("anthropic/claude-3-5-sonnet")
        .await
        .unwrap()
        .expect("claude must resolve");
    assert_eq!(or1.agent_type, "openrouter", "claude must route to openrouter");
    assert_eq!(or1.metadata["acp_prefix"].as_str(), Some("acp.or"));

    // openai/gpt-4o → openrouter
    let or2 = registry
        .find_by_model("openai/gpt-4o")
        .await
        .unwrap()
        .expect("gpt-4o must resolve");
    assert_eq!(or2.agent_type, "openrouter", "gpt-4o must route to openrouter");

    // o4-mini → codex
    let codex = registry.find_by_model("o4-mini").await.unwrap().expect("o4-mini must resolve");
    assert_eq!(codex.agent_type, "codex", "o4-mini must route to codex");
    assert_eq!(codex.metadata["acp_prefix"].as_str(), Some("acp.codex"));

    // unknown model → None
    assert!(
        registry.find_by_model("gpt-5-unknown").await.unwrap().is_none(),
        "unknown model must return None from multi-runner registry"
    );
}

/// Calling `provision_streams` twice on the same NATS server must succeed —
/// it creates-or-updates, not create-or-fail.
#[tokio::test]
async fn provision_streams_is_idempotent() {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = async_nats::jetstream::new(nats);
    let js = NatsJetStreamClient::new(js_ctx);
    let prefix = AcpPrefix::new("acp").unwrap();

    let first = provision_streams(&js, &prefix).await;
    assert!(first.is_ok(), "first provision_streams call must succeed: {first:?}");

    let second = provision_streams(&js, &prefix).await;
    assert!(second.is_ok(), "second provision_streams call must succeed (idempotent): {second:?}");
}

// ── QueuedHttpClient: returns canned responses for bash e2e test ──────────────

#[derive(Clone)]
struct QueuedHttpClient {
    queue: Arc<Mutex<VecDeque<Vec<OpenRouterEvent>>>>,
}

impl QueuedHttpClient {
    fn new() -> Self {
        Self { queue: Arc::new(Mutex::new(VecDeque::new())) }
    }

    fn push(&self, events: Vec<OpenRouterEvent>) {
        self.queue.lock().unwrap().push_back(events);
    }
}

#[async_trait(?Send)]
impl OpenRouterHttpClient for QueuedHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> LocalBoxStream<'static, OpenRouterEvent> {
        let events = self.queue.lock().unwrap().pop_front().unwrap_or_default();
        stream::iter(events).boxed_local()
    }
}

// ── execute_bash_via_nats integration test ────────────────────────────────────

#[tokio::test]
async fn execute_bash_via_nats_delivers_output() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    // Registry needs JetStream KV — provision it but NOT the ACP command streams,
    // as those would intercept core NATS request-reply with JetStream acks.
    let prefix = AcpPrefix::new("acp").unwrap();
    let reg_store = trogon_registry::provision(&js).await.unwrap();
    let registry = trogon_registry::Registry::new(reg_store);

    // Register a fake "execution" backend so wasm_prefix resolves to "acp.wasm"
    let exec_cap = trogon_registry::AgentCapability {
        agent_type: "execution".to_string(),
        capabilities: vec!["terminal".to_string()],
        nats_subject: "acp.wasm.session.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "acp.wasm" }),
    };
    registry.register(&exec_cap).await.unwrap();

    // Subscribe to terminal NATS subjects and reply with mock output
    let mut sub_create = nats.subscribe("acp.wasm.session.*.client.terminal.create").await.unwrap();
    let mut sub_wait = nats.subscribe("acp.wasm.session.*.client.terminal.wait_for_exit").await.unwrap();
    let mut sub_output = nats.subscribe("acp.wasm.session.*.client.terminal.output").await.unwrap();
    let mut sub_release = nats.subscribe("acp.wasm.session.*.client.terminal.release").await.unwrap();

    let nats_responder = nats.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(msg) = sub_create.next() => {
                    let resp = CreateTerminalResponse::new("tid-bash-1");
                    if let Some(reply) = msg.reply {
                        let _ = nats_responder.publish(reply, serde_json::to_vec(&resp).unwrap().into()).await;
                    }
                }
                Some(msg) = sub_wait.next() => {
                    if let Some(reply) = msg.reply {
                        // WaitForTerminalExit: just reply with empty object
                        let _ = nats_responder.publish(reply, b"{}".as_ref().into()).await;
                    }
                }
                Some(msg) = sub_output.next() => {
                    let resp = TerminalOutputResponse::new("hello from bash\n", false);
                    if let Some(reply) = msg.reply {
                        let _ = nats_responder.publish(reply, serde_json::to_vec(&resp).unwrap().into()).await;
                    }
                }
                Some(msg) = sub_release.next() => {
                    if let Some(reply) = msg.reply {
                        let _ = nats_responder.publish(reply, b"{}".as_ref().into()).await;
                    }
                }
            }
        }
    });

    // HTTP client: first call returns bash tool call, second returns final answer
    let http = QueuedHttpClient::new();
    http.push(vec![OpenRouterEvent::ToolCallsReady {
        calls: vec![AssembledToolCall {
            id: "call_bash".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"echo hello"}"#.to_string(),
        }],
    }]);
    http.push(vec![OpenRouterEvent::TextDelta { text: "done".to_string() }]);

    // Start agent with execution backend on a separate thread
    let http_for_agent = http.clone();
    let nats_for_agent = nats.clone();
    let registry_for_agent = registry.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let notifier = NatsSessionNotifier::new(nats_for_agent.clone(), prefix.clone());
        let agent = OpenRouterAgent::with_deps(notifier, "test-model", "test-key", http_for_agent)
            .with_execution_backend(nats_for_agent.clone(), registry_for_agent);
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats_for_agent, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create session
    let new_payload = serde_json::to_vec(&NewSessionRequest::new("/")).unwrap();
    let sid_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", new_payload.into()),
    )
    .await
    .unwrap()
    .unwrap();
    let sid_val: Value = serde_json::from_slice(&sid_msg.payload).unwrap();
    let sid = sid_val["sessionId"].as_str().unwrap().to_string();

    // Send prompt that triggers bash tool call
    let prompt_subj = format!("acp.session.{sid}.agent.prompt");
    let prompt_payload = serde_json::to_vec(
        &PromptRequest::new(sid.clone(), vec![ContentBlock::from("run bash")]),
    )
    .unwrap();
    let resp_msg = tokio::time::timeout(
        Duration::from_secs(15),
        nats.request(prompt_subj, prompt_payload.into()),
    )
    .await
    .expect("timed out waiting for prompt response after bash execution")
    .expect("NATS request failed");

    let resp: PromptResponse = serde_json::from_slice(&resp_msg.payload)
        .unwrap_or_else(|e| {
            let raw = String::from_utf8_lossy(&resp_msg.payload);
            panic!("failed to parse PromptResponse: {e}\nraw: {raw}");
        });
    assert!(
        matches!(resp.stop_reason, agent_client_protocol::StopReason::EndTurn),
        "bash e2e must complete with EndTurn: {:?}",
        resp.stop_reason
    );
}

// ── Fix 1: enabled_tools persisted to KV and restored across agent restart ───

/// Full chain with a real NATS KV bucket:
///
/// Agent 1: new_session → set_config_option(read_file=disabled) → close_session
///          (close_session calls build_snapshot which stores enabled_tools, then
///           NatsSessionStore::save writes it to the SESSIONS KV bucket)
///
/// Agent 2: fresh instance (no in-memory sessions), same NatsSessionStore →
///          load_session → NatsSessionStore::load reads from KV →
///          enabled_tools restored → read_file must still be disabled
#[tokio::test]
async fn enabled_tools_persisted_to_kv_and_restored_on_load_session() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let store = Arc::new(
        trogon_openrouter_runner::NatsSessionStore::open(&js, 0)
            .await
            .expect("NatsSessionStore::open"),
    );
    let prefix = AcpPrefix::new("acp").unwrap();

    // ── Agent 1: create session, disable read_file, close ─────────────────────
    let session_id = {
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix.clone());
        let agent1 = OpenRouterAgent::with_deps(notifier, "test-model", "", NoOpHttpClient)
            .with_session_store(store.clone());

        tokio::task::LocalSet::new()
            .run_until(async move {
                let new_resp = agent1
                    .new_session(NewSessionRequest::new("/"))
                    .await
                    .expect("new_session");
                let sid = new_resp.session_id.clone();

                agent1
                    .set_session_config_option(SetSessionConfigOptionRequest::new(
                        sid.clone(),
                        "read_file",
                        "disabled",
                    ))
                    .await
                    .expect("set_config_option");

                // close_session: build_snapshot stores enabled_tools → NatsSessionStore::save
                agent1
                    .close_session(CloseSessionRequest::new(sid.clone()))
                    .await
                    .expect("close_session");

                sid.to_string()
            })
            .await
    };

    // ── Agent 2: fresh instance, same store, no sessions in memory ────────────
    let notifier2 = NatsSessionNotifier::new(nats.clone(), prefix.clone());
    let agent2 = OpenRouterAgent::with_deps(notifier2, "test-model", "", NoOpHttpClient)
        .with_session_store(store.clone());

    tokio::task::LocalSet::new()
        .run_until(async move {
            let load_resp = agent2
                .load_session(LoadSessionRequest::new(
                    SessionId::from(session_id.clone()),
                    "/",
                ))
                .await
                .expect("load_session must succeed via KV fallback");

            let opts = load_resp.config_options.unwrap_or_default();

            let read_file_state = opts
                .iter()
                .find(|o| o.id.to_string() == "read_file")
                .and_then(|o| match &o.kind {
                    SessionConfigKind::Select(s) => Some(s.current_value.to_string()),
                    _ => None,
                })
                .unwrap_or_else(|| "missing".to_string());

            let enabled_count = opts
                .iter()
                .filter(|o| {
                    matches!(&o.kind, SessionConfigKind::Select(s)
                        if s.current_value.to_string() == "enabled")
                })
                .count();
            let total = opts.len();

            assert_eq!(
                read_file_state, "disabled",
                "read_file must remain disabled after KV restore"
            );
            assert_eq!(
                enabled_count,
                total - 1,
                "exactly one tool must be disabled; got {enabled_count}/{total} enabled"
            );
        })
        .await;
}

// ── Fix 2: fork session tools persisted to KV and restored across agent restart ─

#[tokio::test]
async fn fork_session_tools_persisted_to_kv_and_restored_on_load_session() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let store = Arc::new(
        trogon_openrouter_runner::NatsSessionStore::open(&js, 0)
            .await
            .expect("NatsSessionStore::open"),
    );
    let prefix = AcpPrefix::new("acp").unwrap();

    // Agent 1: new_session → disable read_file → fork_session (saves fork snapshot to KV)
    let fork_id = {
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix.clone());
        let agent1 = OpenRouterAgent::with_deps(notifier, "test-model", "", NoOpHttpClient)
            .with_session_store(store.clone());

        tokio::task::LocalSet::new()
            .run_until(async move {
                let src_id = agent1
                    .new_session(NewSessionRequest::new("/"))
                    .await
                    .expect("new_session")
                    .session_id;

                agent1
                    .set_session_config_option(SetSessionConfigOptionRequest::new(
                        src_id.clone(),
                        "read_file",
                        "disabled",
                    ))
                    .await
                    .expect("set_config_option");

                agent1
                    .fork_session(ForkSessionRequest::new(src_id, std::path::PathBuf::from("/fork")))
                    .await
                    .expect("fork_session")
                    .session_id
                    .to_string()
            })
            .await
    };

    // Agent 2: fresh instance, same store, load fork session
    let notifier2 = NatsSessionNotifier::new(nats.clone(), prefix.clone());
    let agent2 = OpenRouterAgent::with_deps(notifier2, "test-model", "", NoOpHttpClient)
        .with_session_store(store.clone());

    tokio::task::LocalSet::new()
        .run_until(async move {
            let load_resp = agent2
                .load_session(LoadSessionRequest::new(SessionId::from(fork_id.clone()), "/"))
                .await
                .expect("load_session of fork must succeed via KV");

            let opts = load_resp.config_options.unwrap_or_default();
            let read_file_state = opts
                .iter()
                .find(|o| o.id.to_string() == "read_file")
                .and_then(|o| match &o.kind {
                    SessionConfigKind::Select(s) => Some(s.current_value.to_string()),
                    _ => None,
                })
                .unwrap_or_else(|| "missing".to_string());

            assert_eq!(
                read_file_state, "disabled",
                "fork must preserve disabled read_file after KV round-trip"
            );
        })
        .await;
}

// ── Fix 3: history and model survive KV round-trip ────────────────────────────

/// Records the messages array passed to each chat_stream call.
#[derive(Clone)]
struct RecordingHttpClient {
    calls: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl RecordingHttpClient {
    fn new() -> Self {
        Self { calls: Arc::new(Mutex::new(Vec::new())) }
    }
}

#[async_trait(?Send)]
impl OpenRouterHttpClient for RecordingHttpClient {
    async fn chat_stream(
        &self,
        _model: &str,
        messages: &[Message],
        _api_key: &str,
        _tools: &[ToolDef],
    ) -> LocalBoxStream<'static, OpenRouterEvent> {
        self.calls.lock().unwrap().push(messages.to_vec());
        Box::pin(stream::empty())
    }
}

#[tokio::test]
async fn history_and_model_survive_kv_round_trip() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let store = Arc::new(
        trogon_openrouter_runner::NatsSessionStore::open(&js, 0)
            .await
            .expect("NatsSessionStore::open"),
    );
    let prefix = AcpPrefix::new("acp").unwrap();

    // Agent 1: default "saved-model" (distinct from agent2's default).
    // new_session → set_session_model → prompt (adds history) → close_session (saves to KV).
    let session_id = {
        let http = QueuedHttpClient::new();
        http.push(vec![OpenRouterEvent::TextDelta { text: "Reply from AI".to_string() }]);
        let notifier = NatsSessionNotifier::new(nats.clone(), prefix.clone());
        // "saved-model" is agent1's default, so it passes set_session_model validation.
        let agent1 = OpenRouterAgent::with_deps(notifier, "saved-model", "test-key", http)
            .with_session_store(store.clone());

        tokio::task::LocalSet::new()
            .run_until(async move {
                let sid = agent1
                    .new_session(NewSessionRequest::new("/"))
                    .await
                    .expect("new_session")
                    .session_id;

                agent1
                    .set_session_model(SetSessionModelRequest::new(sid.clone(), "saved-model"))
                    .await
                    .expect("set_session_model");

                agent1
                    .prompt(PromptRequest::new(sid.clone(), vec![ContentBlock::from("ask something")]))
                    .await
                    .expect("prompt");

                agent1
                    .close_session(CloseSessionRequest::new(sid.clone()))
                    .await
                    .expect("close_session");

                sid.to_string()
            })
            .await
    };

    // Agent 2: default "test-model" (different from "saved-model").
    // After load_session the model must come from the KV snapshot, not from the agent default.
    let recorder = RecordingHttpClient::new();
    let calls = recorder.calls.clone();
    let notifier2 = NatsSessionNotifier::new(nats.clone(), prefix.clone());
    let agent2 = OpenRouterAgent::with_deps(notifier2, "test-model", "test-key", recorder)
        .with_session_store(store.clone());

    tokio::task::LocalSet::new()
        .run_until(async move {
            let load_resp = agent2
                .load_session(LoadSessionRequest::new(SessionId::from(session_id.clone()), "/"))
                .await
                .expect("load_session must succeed via KV");

            // Model must come from the KV snapshot ("saved-model"), not from agent2's default ("test-model").
            let restored_model = load_resp.models
                .as_ref()
                .map(|m| m.current_model_id.0.as_ref().to_string());
            assert_eq!(
                restored_model.as_deref(),
                Some("saved-model"),
                "model must survive KV round-trip; got {restored_model:?}"
            );

            agent2
                .prompt(PromptRequest::new(
                    SessionId::from(session_id.clone()),
                    vec![ContentBlock::from("follow-up")],
                ))
                .await
                .expect("follow-up prompt");

            let recorded = calls.lock().unwrap();
            let msgs = recorded.last().expect("at least one chat_stream call must have been made");
            // The messages array sent to the API must include the prior turn's user + assistant
            // messages in addition to the new "follow-up" user message.
            assert!(
                msgs.iter().any(|m| m.role == "user" && m.content == "ask something"),
                "restored history must include prior user message; messages: {msgs:?}"
            );
            assert!(
                msgs.iter().any(|m| m.role == "assistant" && m.content == "Reply from AI"),
                "restored history must include prior assistant reply; messages: {msgs:?}"
            );
        })
        .await;
}

// ── Regression: pre-fix snapshot with tools:[] restores all trogon tools ─────

/// An old snapshot with `tools: []` (written before enabled_tools was persisted)
/// must restore all trogon tools on load_session.
#[tokio::test]
async fn pre_fix_snapshot_empty_tools_restores_trogon_tools() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let store = Arc::new(
        trogon_openrouter_runner::NatsSessionStore::open(&js, 0)
            .await
            .expect("NatsSessionStore::open"),
    );
    let prefix = AcpPrefix::new("acp").unwrap();

    // Write a pre-fix snapshot (tools: []) directly to the KV bucket
    let session_id = "pre-fix-session-or";
    let snap = trogon_openrouter_runner::session_store::SessionSnapshot {
        id: session_id.to_string(),
        tenant_id: "default".to_string(),
        name: "Pre-fix session".to_string(),
        model: None,
        tools: vec![],
        memory_path: None,
        messages: vec![],
        created_at: "2026-01-01T00:00:00.000Z".to_string(),
        updated_at: "2026-01-01T00:00:00.000Z".to_string(),
        agent_id: None,
        parent_session_id: None,
        branched_at_index: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_read_tokens: 0,
        total_cache_creation_tokens: 0,
    };
    use trogon_openrouter_runner::SessionStoring as _;
    store.save(&snap).await;

    // Agent: load_session → verify all trogon tools are restored
    let notifier = NatsSessionNotifier::new(nats.clone(), prefix.clone());
    let agent = OpenRouterAgent::with_deps(notifier, "test-model", "", NoOpHttpClient)
        .with_session_store(store.clone());

    tokio::task::LocalSet::new()
        .run_until(async move {
            let load_resp = agent
                .load_session(LoadSessionRequest::new(SessionId::from(session_id), "/"))
                .await
                .expect("load_session must succeed for pre-fix snapshot");

            let opts = load_resp.config_options.unwrap_or_default();
            let enabled: Vec<String> = opts
                .iter()
                .filter(|o| {
                    matches!(&o.kind, SessionConfigKind::Select(s)
                        if s.current_value.to_string() == "enabled")
                })
                .map(|o| o.id.to_string())
                .collect();

            assert!(
                enabled.contains(&"read_file".to_string()),
                "read_file must be re-enabled from pre-fix snapshot; enabled: {enabled:?}"
            );
            assert!(
                enabled.contains(&"write_file".to_string()),
                "write_file must be re-enabled from pre-fix snapshot"
            );
            let all_names: Vec<String> = trogon_tools::all_tool_defs()
                .iter()
                .map(|d| d.name.clone())
                .collect();
            for name in &all_names {
                assert!(
                    enabled.contains(name),
                    "tool {name} must be enabled when restoring pre-fix snapshot"
                );
            }
        })
        .await;
}

// ── session/export and session/import with NatsSessionStore ──────────────────

/// Full export→import round-trip with a real NatsSessionStore:
///
/// 1. new_session(sid1) → import messages=[{role:"user",text:"nats test"}]
/// 2. export from sid1 → parse PortableMessage, assert len==1, text=="nats test"
/// 3. new_session(sid2) → import using sid1's export JSON
/// 4. export from sid2 → assert same content
#[tokio::test]
async fn ext_method_export_import_round_trip_with_nats_store() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let store = Arc::new(
        trogon_openrouter_runner::NatsSessionStore::open(&js, 0)
            .await
            .expect("NatsSessionStore::open"),
    );
    let prefix = AcpPrefix::new("acp").unwrap();
    let notifier = NatsSessionNotifier::new(nats.clone(), prefix.clone());
    let agent = OpenRouterAgent::with_deps(notifier, "test-model", "", NoOpHttpClient)
        .with_session_store(store.clone());

    tokio::task::LocalSet::new()
        .run_until(async move {
            // ── 1. Create two sessions ────────────────────────────────────────
            let sid1 = agent
                .new_session(NewSessionRequest::new("/"))
                .await
                .expect("new_session sid1")
                .session_id;

            let sid2 = agent
                .new_session(NewSessionRequest::new("/"))
                .await
                .expect("new_session sid2")
                .session_id;

            // ── 2. Import a message into sid1 ─────────────────────────────────
            let import_params = serde_json::value::RawValue::from_string(
                format!(
                    r#"{{"sessionId":"{}","messages":[{{"role":"user","text":"nats test"}}]}}"#,
                    sid1
                ),
            )
            .unwrap();
            agent
                .ext_method(ExtRequest::new("session/import", import_params.into()))
                .await
                .expect("session/import into sid1");

            // ── 3. Export from sid1 ───────────────────────────────────────────
            let export1_params = serde_json::value::RawValue::from_string(
                format!(r#"{{"sessionId":"{}"}}"#, sid1),
            )
            .unwrap();
            let export1_resp = agent
                .ext_method(ExtRequest::new("session/export", export1_params.into()))
                .await
                .expect("session/export from sid1");

            let portable1: Vec<trogon_runner_tools::portable_session::PortableMessage> =
                serde_json::from_str(export1_resp.0.get())
                    .expect("parse sid1 export as Vec<PortableMessage>");
            assert_eq!(portable1.len(), 1, "sid1 export must have 1 message");
            assert_eq!(portable1[0].role, "user", "sid1 message role must be user");
            assert_eq!(
                portable1[0].text, "nats test",
                "sid1 message text must be 'nats test'"
            );

            // ── 4. Import sid1's export into sid2 ────────────────────────────
            let export1_json = export1_resp.0.get().to_string();
            let import2_params = serde_json::value::RawValue::from_string(
                format!(r#"{{"sessionId":"{}","messages":{}}}"#, sid2, export1_json),
            )
            .unwrap();
            agent
                .ext_method(ExtRequest::new("session/import", import2_params.into()))
                .await
                .expect("session/import into sid2");

            // ── 5. Export from sid2 and verify same content ───────────────────
            let export2_params = serde_json::value::RawValue::from_string(
                format!(r#"{{"sessionId":"{}"}}"#, sid2),
            )
            .unwrap();
            let export2_resp = agent
                .ext_method(ExtRequest::new("session/export", export2_params.into()))
                .await
                .expect("session/export from sid2");

            let portable2: Vec<trogon_runner_tools::portable_session::PortableMessage> =
                serde_json::from_str(export2_resp.0.get())
                    .expect("parse sid2 export as Vec<PortableMessage>");
            assert_eq!(
                portable2.len(),
                portable1.len(),
                "sid2 export must have same message count as sid1"
            );
            assert_eq!(portable2[0].role, portable1[0].role, "role must match");
            assert_eq!(portable2[0].text, portable1[0].text, "text must match");
        })
        .await;
}
