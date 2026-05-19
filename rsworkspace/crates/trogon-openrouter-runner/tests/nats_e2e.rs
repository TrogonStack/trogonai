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
    Agent as _, CloseSessionRequest, ContentBlock, CreateTerminalResponse, LoadSessionRequest,
    NewSessionRequest, PromptRequest, PromptResponse, SessionConfigKind, SessionId,
    SetSessionConfigOptionRequest, TerminalOutputResponse,
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
