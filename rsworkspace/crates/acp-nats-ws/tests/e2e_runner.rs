//! End-to-end integration tests: WebSocket bridge + real TrogonAgent + real NATS.
//!
//! These tests verify the full ACP request-reply flow:
//!   WS client → acp-nats-ws → NATS → TrogonAgent (trogon-acp-runner) → back
//!
//! Requires Docker (testcontainers starts a NATS server with JetStream).
//!
//! Run with:
//!   cargo test -p acp-nats-ws --test e2e_runner

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
use acp_nats_agent::AgentSideNatsConnection;
use acp_nats_ws::upgrade::{ConnectionRequest, UpgradeState};
use acp_nats_ws::{THREAD_NAME, run_connection_thread, upgrade};
use async_nats::jetstream;
use futures_util::{SinkExt, StreamExt};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock, mpsc, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use trogon_acp_runner::{NatsSessionNotifier, NatsSessionStore, SessionStore as _, TrogonAgent};
use trogon_agent_core::agent_loop::AgentLoop;
use trogon_agent_core::tools::ToolContext;
use trogon_codex_runner::DefaultCodexAgent;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_xai_runner::{MockXaiHttpClient, NatsSessionNotifier as XaiSessionNotifier, XaiAgent, XaiEvent};

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (
    ContainerAsync<Nats>,
    async_nats::Client,
    jetstream::Context,
    u16,
) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js = jetstream::new(nats.clone());
    (container, nats, js, port)
}

fn make_config(nats_port: u16) -> Config {
    Config::new(
        AcpPrefix::new("acp").unwrap(),
        NatsConfig {
            servers: vec![format!("127.0.0.1:{nats_port}")],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_secs(5))
}

fn make_agent_loop() -> AgentLoop {
    let http = reqwest::Client::new();
    AgentLoop {
        http_client: http.clone(),
        proxy_url: String::new(),
        anthropic_token: String::new(),
        anthropic_base_url: None,
        anthropic_extra_headers: vec![],
        model: "claude-opus-4-6".to_string(),
        max_iterations: 10,
        thinking_budget: None,
        tool_context: Arc::new(ToolContext { proxy_url: String::new() }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
    }
}

async fn start_rpc_server(nats: async_nats::Client, js: jetstream::Context) -> NatsSessionStore {
    // Open a verification store in the outer (test) runtime.
    let verification_store = NatsSessionStore::open(&js).await.unwrap();
    let gateway_config = Arc::new(RwLock::new(None));

    // The agent runs in its own thread with its own Tokio runtime.
    // Its NatsSessionStore must be opened inside that runtime to avoid cross-runtime issues.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let store = rt.block_on(async {
            let js = jetstream::new(nats.clone());
            NatsSessionStore::open(&js).await.unwrap()
        });
        let ta = TrogonAgent::new(
            NatsSessionNotifier::new(nats.clone()),
            store,
            make_agent_loop(),
            "acp",
            "claude-opus-4-6",
            None,
            None,
            gateway_config,
        );
        let prefix = AcpPrefix::new("acp").unwrap();
        let (_, io_task) = AgentSideNatsConnection::new(ta, nats, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    verification_store
}

async fn start_ws_server(
    nats_port: u16,
) -> (String, watch::Sender<bool>, std::thread::JoinHandle<()>) {
    let nats_client = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("connect to NATS for WS bridge");
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(
        async_nats::jetstream::new(nats_client.clone()),
    );
    let config = make_config(nats_port);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

    let conn_thread = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || run_connection_thread(conn_rx, nats_client, js_client, config))
        .expect("failed to spawn connection thread");

    let state = UpgradeState {
        conn_tx,
        shutdown_tx: shutdown_tx.clone(),
    };

    let app = axum::Router::new()
        .route("/ws", axum::routing::get(upgrade::handle))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.changed().await;
            })
            .await
            .unwrap();
    });

    (format!("ws://{addr}/ws"), shutdown_tx, conn_thread)
}

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// Read the next Text message from a WS stream, skipping non-Text frames.
async fn next_text(ws: &mut WsStream) -> String {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return t.to_string(),
            Some(Ok(_)) => continue,
            other => panic!("unexpected ws message: {other:?}"),
        }
    }
}

/// Read text frames until finding one whose `"id"` field matches `expected_id`.
/// Skips notifications (no `"id"` field) and mismatched responses.
async fn next_with_id(ws: &mut WsStream, expected_id: i64) -> serde_json::Value {
    for _ in 0..100usize {
        let text = tokio::time::timeout(Duration::from_secs(10), next_text(ws))
            .await
            .expect("timeout waiting for WS frame");
        let val: serde_json::Value = serde_json::from_str(&text).unwrap();
        if val["id"].as_i64() == Some(expected_id) {
            return val;
        }
    }
    panic!("did not receive response with id={expected_id} after 100 frames");
}

/// Create all required JetStream streams for the ACP protocol.
async fn setup_streams(js: &jetstream::Context) {
    let prefix = AcpPrefix::new("acp").unwrap();
    for config in acp_nats::jetstream::streams::all_configs(&prefix) {
        js.get_or_create_stream(config).await.unwrap();
    }
}

/// Start an XaiAgent backed by a MockXaiHttpClient on its own thread.
async fn start_xai_agent(nats: async_nats::Client, mock: Arc<MockXaiHttpClient>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        rt.block_on(local.run_until(async move {
            let prefix = AcpPrefix::new("acp").unwrap();
            let js_client = NatsJetStreamClient::new(jetstream::new(nats.clone()));
            let notifier = XaiSessionNotifier::new(nats.clone(), prefix.clone());
            let agent = XaiAgent::with_deps(notifier, "grok-4", "dummy", mock);
            let (_, io_task) = AgentSideNatsConnection::with_jetstream(
                agent,
                nats,
                js_client,
                prefix,
                |fut| { tokio::task::spawn_local(fut); },
            );
            io_task.await.ok();
        }));
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
}

static CODEX_BIN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn codex_bin_lock() -> &'static Mutex<()> {
    CODEX_BIN_LOCK.get_or_init(Mutex::default)
}

fn mock_codex_bin() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    let bin = exe.parent().unwrap().parent().unwrap().join("mock_codex_server");
    assert!(
        bin.exists(),
        "mock_codex_server not found at {bin:?} — run: cargo build -p trogon-codex-runner"
    );
    bin
}

async fn start_codex_agent(nats: async_nats::Client) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        rt.block_on(local.run_until(async move {
            let prefix = AcpPrefix::new("acp").unwrap();
            let js_client = NatsJetStreamClient::new(jetstream::new(nats.clone()));
            let agent = DefaultCodexAgent::with_nats(nats.clone(), prefix.clone(), "o4-mini");
            let (_, io_task) = AgentSideNatsConnection::with_jetstream(
                agent,
                nats,
                js_client,
                prefix,
                |fut| { tokio::task::spawn_local(fut); },
            );
            io_task.await.ok();
        }));
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Full E2E: WS client → bridge → NATS → TrogonAgent → back.
/// TrogonAgent handles `initialize` and returns capabilities.
#[tokio::test]
async fn e2e_initialize_returns_protocol_version_and_capabilities() {
    let (_container, nats, js, nats_port) = start_nats().await;
    let _ = start_rpc_server(nats, js).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#;
    ws.send(Message::Text(req.into())).await.unwrap();

    let text = tokio::time::timeout(Duration::from_secs(10), next_text(&mut ws))
        .await
        .expect("timed out waiting for initialize response");

    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["id"], 1, "response id must match request id");
    assert!(
        val["result"]["protocolVersion"].is_number(),
        "must have protocolVersion: {text}"
    );
    assert!(
        val["result"]["agentCapabilities"]["loadSession"]
            .as_bool()
            .unwrap_or(false),
        "must advertise loadSession: {text}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E new_session: bridge → NATS → TrogonAgent creates session → client gets session ID.
#[tokio::test]
async fn e2e_new_session_returns_session_id() {
    let (_container, nats, js, nats_port) = start_nats().await;
    let store = start_rpc_server(nats, js).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let req = r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#;
    ws.send(Message::Text(req.into())).await.unwrap();

    let text = tokio::time::timeout(Duration::from_secs(10), next_text(&mut ws))
        .await
        .expect("timed out waiting for session/new response");

    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["id"], 2);
    let session_id = val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId in response: {text}"));
    assert!(!session_id.is_empty(), "sessionId must not be empty");

    // Verify the session was persisted in the store.
    let state = store.load(session_id).await.unwrap();
    assert_eq!(state.cwd, "/tmp");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E list_sessions: after creating two sessions, listing returns both.
#[tokio::test]
async fn e2e_list_sessions_returns_created_sessions() {
    let (_container, nats, js, nats_port) = start_nats().await;
    let _ = start_rpc_server(nats, js).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Create two sessions.
    for (id, cwd) in [(3, "/proj1"), (4, "/proj2")] {
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"session/new","params":{{"cwd":"{cwd}","mcpServers":[]}}}}"#
        );
        ws.send(Message::Text(req.into())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), next_text(&mut ws))
            .await
            .expect("timed out waiting for session/new");
    }

    // List sessions.
    let req = r#"{"jsonrpc":"2.0","id":5,"method":"session/list","params":{}}"#;
    ws.send(Message::Text(req.into())).await.unwrap();
    let text = tokio::time::timeout(Duration::from_secs(10), next_text(&mut ws))
        .await
        .expect("timed out waiting for session/list");

    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["id"], 5);
    let sessions = val["result"]["sessions"]
        .as_array()
        .expect("must have sessions array");
    assert_eq!(sessions.len(), 2, "expected 2 sessions: {text}");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E authenticate: bridge routes authenticate to TrogonAgent, which replies with empty response.
#[tokio::test]
async fn e2e_authenticate_returns_ok() {
    let (_container, nats, js, nats_port) = start_nats().await;
    let _ = start_rpc_server(nats, js).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let req =
        r#"{"jsonrpc":"2.0","id":6,"method":"authenticate","params":{"methodId":"password"}}"#;
    ws.send(Message::Text(req.into())).await.unwrap();

    let text = tokio::time::timeout(Duration::from_secs(10), next_text(&mut ws))
        .await
        .expect("timed out waiting for authenticate response");

    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["id"], 6);
    assert!(val["result"].is_object(), "must have result: {text}");
    assert!(val["error"].is_null(), "must not have error: {text}");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS client → bridge → NATS JetStream → XaiAgent (mock HTTP) → prompt response.
/// Verifies the full session/prompt path over the WebSocket transport end-to-end.
#[tokio::test]
async fn e2e_ws_prompt_returns_stop_reason() {
    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;

    let mock = Arc::new(MockXaiHttpClient::default());
    mock.push_response(vec![
        XaiEvent::TextDelta { text: "hello from ws".to_string() },
        XaiEvent::Done,
    ]);
    start_xai_agent(nats, mock).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // initialize
    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    let init_val = next_with_id(&mut ws, 1).await;
    assert!(
        init_val["result"]["protocolVersion"].is_number(),
        "must have protocolVersion: {init_val}"
    );

    // session/new
    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();
    assert!(!session_id.is_empty());

    // session/prompt
    let prompt_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "ping"}]
        }
    });
    ws.send(Message::Text(prompt_msg.to_string().into()))
        .await
        .unwrap();

    let prompt_val = next_with_id(&mut ws, 3).await;
    assert!(
        prompt_val["result"]["stopReason"].is_string(),
        "prompt result must have stopReason: {prompt_val}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS client creates a session then loads it — verifies the full reconnect path.
/// TrogonAgent stores session state in JetStream KV; session/load restores it.
#[tokio::test]
async fn e2e_ws_session_load_reconnects_to_existing_session() {
    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    let mock = Arc::new(MockXaiHttpClient::default());
    start_xai_agent(nats.clone(), mock).await;
    let _ = start_rpc_server(nats, js).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // initialize
    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    // session/new
    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    // session/load — reconnect to the session that was just created
    let load_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/load",
        "params": {
            "sessionId": session_id,
            "cwd": "/tmp",
            "mcpServers": []
        }
    });
    ws.send(Message::Text(load_msg.to_string().into()))
        .await
        .unwrap();

    let load_val = next_with_id(&mut ws, 3).await;
    assert!(
        load_val["result"].is_object(),
        "session/load must return a result: {load_val}"
    );
    assert!(
        load_val["error"].is_null(),
        "session/load must not return an error: {load_val}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS session/cancel notification is delivered while prompt is in flight.
/// The cancel is sent immediately after the prompt; the prompt must still return
/// a valid stopReason (either completed or cancelled — both are fine).
#[tokio::test]
async fn e2e_ws_cancel_accepted_and_prompt_completes() {
    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    let mock = Arc::new(MockXaiHttpClient::default());
    mock.push_response(vec![
        XaiEvent::TextDelta { text: "pong".to_string() },
        XaiEvent::Done,
    ]);
    start_xai_agent(nats, mock).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let prompt_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/prompt",
        "params": { "sessionId": session_id, "prompt": [{"type": "text", "text": "ping"}] }
    });
    ws.send(Message::Text(prompt_msg.to_string().into())).await.unwrap();

    let cancel_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    });
    ws.send(Message::Text(cancel_msg.to_string().into())).await.unwrap();

    let prompt_val = next_with_id(&mut ws, 3).await;
    assert!(
        prompt_val["result"]["stopReason"].is_string(),
        "prompt result must have stopReason: {prompt_val}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS session/close terminates the session — the response must be a
/// result object with no error.
#[tokio::test]
async fn e2e_ws_session_close_terminates_session() {
    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    let mock = Arc::new(MockXaiHttpClient::default());
    start_xai_agent(nats, mock).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let close_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/close",
        "params": { "sessionId": session_id }
    });
    ws.send(Message::Text(close_msg.to_string().into())).await.unwrap();

    let close_val = next_with_id(&mut ws, 3).await;
    assert!(
        close_val["result"].is_object(),
        "session/close must return a result: {close_val}"
    );
    assert!(
        close_val["error"].is_null(),
        "session/close must not return an error: {close_val}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS session/fork creates a new child session with a different ID.
#[tokio::test]
async fn e2e_ws_session_fork_creates_child_session() {
    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    let mock = Arc::new(MockXaiHttpClient::default());
    start_xai_agent(nats, mock).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let fork_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/fork",
        "params": { "sessionId": session_id, "cwd": "/tmp", "mcpServers": [] }
    });
    ws.send(Message::Text(fork_msg.to_string().into())).await.unwrap();

    let fork_val = next_with_id(&mut ws, 3).await;
    assert!(
        fork_val["error"].is_null(),
        "session/fork must not return an error: {fork_val}"
    );
    let new_session_id = fork_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/fork must return new sessionId: {fork_val}"));
    assert!(!new_session_id.is_empty());
    assert_ne!(new_session_id, session_id);

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS → DefaultCodexAgent (mock binary) → prompt returns a stopReason.
#[tokio::test]
async fn e2e_ws_codex_prompt_returns_stop_reason() {
    let _lock = codex_bin_lock().lock().await;
    // SAFETY: serialized by CODEX_BIN_LOCK within this test binary.
    unsafe { std::env::set_var("CODEX_BIN", mock_codex_bin()) };

    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    start_codex_agent(nats).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let prompt_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/prompt",
        "params": { "sessionId": session_id, "prompt": [{"type": "text", "text": "ping"}] }
    });
    ws.send(Message::Text(prompt_msg.to_string().into())).await.unwrap();

    let prompt_val = next_with_id(&mut ws, 3).await;
    assert!(
        prompt_val["result"]["stopReason"].is_string(),
        "codex prompt must return stopReason: {prompt_val}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS → DefaultCodexAgent → cancel sent immediately after prompt; prompt
/// must still complete with a valid stopReason (completed or cancelled).
#[tokio::test]
async fn e2e_ws_codex_cancel_during_prompt_completes() {
    let _lock = codex_bin_lock().lock().await;
    unsafe { std::env::set_var("CODEX_BIN", mock_codex_bin()) };

    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    start_codex_agent(nats).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let prompt_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/prompt",
        "params": { "sessionId": session_id, "prompt": [{"type": "text", "text": "ping"}] }
    });
    ws.send(Message::Text(prompt_msg.to_string().into())).await.unwrap();

    let cancel_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id }
    });
    ws.send(Message::Text(cancel_msg.to_string().into())).await.unwrap();

    let prompt_val = next_with_id(&mut ws, 3).await;
    assert!(
        prompt_val["result"]["stopReason"].is_string(),
        "prompt must return stopReason after cancel: {prompt_val}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS → DefaultCodexAgent → session/load reconnects to existing session.
#[tokio::test]
async fn e2e_ws_codex_load_session_reconnects() {
    let _lock = codex_bin_lock().lock().await;
    unsafe { std::env::set_var("CODEX_BIN", mock_codex_bin()) };

    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    start_codex_agent(nats).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let load_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/load",
        "params": { "sessionId": session_id, "cwd": "/tmp", "mcpServers": [] }
    });
    ws.send(Message::Text(load_msg.to_string().into())).await.unwrap();

    let load_val = next_with_id(&mut ws, 3).await;
    assert!(
        load_val["result"].is_object(),
        "session/load must return a result: {load_val}"
    );
    assert!(
        load_val["error"].is_null(),
        "session/load must not return an error: {load_val}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS → DefaultCodexAgent → session/fork returns a new child session ID.
#[tokio::test]
async fn e2e_ws_codex_fork_creates_child_session() {
    let _lock = codex_bin_lock().lock().await;
    unsafe { std::env::set_var("CODEX_BIN", mock_codex_bin()) };

    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    start_codex_agent(nats).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let fork_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/fork",
        "params": { "sessionId": session_id, "cwd": "/tmp", "mcpServers": [] }
    });
    ws.send(Message::Text(fork_msg.to_string().into())).await.unwrap();

    let fork_val = next_with_id(&mut ws, 3).await;
    assert!(
        fork_val["error"].is_null(),
        "session/fork must not return an error: {fork_val}"
    );
    let new_session_id = fork_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/fork must return new sessionId: {fork_val}"));
    assert!(!new_session_id.is_empty());
    assert_ne!(new_session_id, session_id);

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS → DefaultCodexAgent → session/close terminates the session.
#[tokio::test]
async fn e2e_ws_codex_close_terminates_session() {
    let _lock = codex_bin_lock().lock().await;
    unsafe { std::env::set_var("CODEX_BIN", mock_codex_bin()) };

    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    start_codex_agent(nats).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/close",
        "params": { "sessionId": session_id }
    });
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();

    let val = next_with_id(&mut ws, 3).await;
    assert!(val["result"].is_object(), "session/close must return a result: {val}");
    assert!(val["error"].is_null(), "session/close must not return an error: {val}");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS → DefaultCodexAgent → session/set_mode returns a result.
#[tokio::test]
async fn e2e_ws_codex_set_session_mode() {
    let _lock = codex_bin_lock().lock().await;
    unsafe { std::env::set_var("CODEX_BIN", mock_codex_bin()) };

    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    start_codex_agent(nats).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/set_mode",
        "params": { "sessionId": session_id, "modeId": "default" }
    });
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();

    let val = next_with_id(&mut ws, 3).await;
    assert!(val["result"].is_object(), "session/set_mode must return a result: {val}");
    assert!(val["error"].is_null(), "session/set_mode must not return an error: {val}");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS → DefaultCodexAgent → session/set_model returns a result.
#[tokio::test]
async fn e2e_ws_codex_set_session_model() {
    let _lock = codex_bin_lock().lock().await;
    unsafe { std::env::set_var("CODEX_BIN", mock_codex_bin()) };

    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    start_codex_agent(nats).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/set_model",
        "params": { "sessionId": session_id, "modelId": "o4-mini" }
    });
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();

    let val = next_with_id(&mut ws, 3).await;
    assert!(val["result"].is_object(), "session/set_model must return a result: {val}");
    assert!(val["error"].is_null(), "session/set_model must not return an error: {val}");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// E2E: WS → XaiAgent → session/resume returns a result after session/new.
#[tokio::test]
async fn e2e_ws_resume_session_returns_result() {
    let (_container, nats, js, nats_port) = start_nats().await;
    setup_streams(&js).await;
    let mock = Arc::new(MockXaiHttpClient::default());
    start_xai_agent(nats, mock).await;
    let (ws_url, shutdown_tx, conn_thread) = start_ws_server(nats_port).await;
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.into(),
    ))
    .await
    .unwrap();
    next_with_id(&mut ws, 1).await;

    ws.send(Message::Text(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#.into(),
    ))
    .await
    .unwrap();
    let new_val = next_with_id(&mut ws, 2).await;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("must have sessionId: {new_val}"))
        .to_string();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "session/resume",
        "params": { "sessionId": session_id, "cwd": "/tmp" }
    });
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();

    let val = next_with_id(&mut ws, 3).await;
    assert!(val["result"].is_object(), "session/resume must return a result: {val}");
    assert!(val["error"].is_null(), "session/resume must not return an error: {val}");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}
