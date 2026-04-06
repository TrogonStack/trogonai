//! End-to-end integration tests: WebSocket bridge + real TrogonAgent + real NATS.
//!
//! These tests verify the full ACP request-reply flow:
//!   WS client → acp-nats-ws → NATS → TrogonAgent (trogon-acp-runner) → back
//!
//! Requires Docker (testcontainers starts a NATS server with JetStream).
//!
//! Run with:
//!   cargo test -p acp-nats-ws --test e2e_runner

use std::sync::Arc;
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
use tokio::sync::{RwLock, mpsc, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use trogon_acp_runner::{SessionStore, TrogonAgent};
use trogon_agent_core::agent_loop::AgentLoop;
use trogon_agent_core::tools::ToolContext;

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
        tool_context: Arc::new(ToolContext { http_client: http, proxy_url: String::new() }),
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
    }
}

async fn start_rpc_server(nats: async_nats::Client, js: jetstream::Context) -> SessionStore {
    let store = SessionStore::open(&js).await.unwrap();
    let gateway_config = Arc::new(RwLock::new(None));
    let store_clone = store.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let ta = TrogonAgent::new(
            nats.clone(),
            store_clone,
            make_agent_loop(),
            "acp",
            "claude-opus-4-6",
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
    store
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

/// Read the next Text message from a WS stream, skipping non-Text frames.
async fn next_text(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> String {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return t.to_string(),
            Some(Ok(_)) => continue,
            other => panic!("unexpected ws message: {other:?}"),
        }
    }
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
