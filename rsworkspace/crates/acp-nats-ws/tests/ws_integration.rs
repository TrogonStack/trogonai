//! Integration tests for acp-nats-ws with a real NATS server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p acp-nats-ws --test ws_integration

use std::sync::Arc;
use std::time::Duration;

use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
use acp_nats_ws::upgrade::{ConnectionRequest, RegistryExtension, UpgradeState};
use acp_nats_ws::{THREAD_NAME, run_connection_thread, upgrade};
use agent_client_protocol::{InitializeResponse, ProtocolVersion};
use futures_util::{SinkExt, StreamExt};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

/// Starts a NATS container with JetStream enabled (`-js` flag), required for
/// the registry KV bucket used by the `/ws/:agent_type` route.
async fn start_nats_with_jetstream() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["-js"])
        .start()
        .await
        .expect("Failed to start NATS+JetStream container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

fn make_config(nats_port: u16) -> Config {
    Config::new(
        AcpPrefix::new("acp").unwrap(),
        NatsConfig {
            servers: vec![format!("127.0.0.1:{nats_port}")],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500))
}

/// Starts the acp-nats-ws server backed by real NATS with only the plain `/ws` route.
async fn start_server(
    nats_port: u16,
) -> (String, watch::Sender<bool>, std::thread::JoinHandle<()>) {
    let nats_client = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("connect to NATS");
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

/// Starts a NATS+JetStream server and the acp-nats-ws server with the
/// `/ws/{agent_type}` route wired to a real NATS registry.
///
/// Returns:
/// - the NATS container (keep alive for the test duration)
/// - the base WebSocket URL (`ws://127.0.0.1:<port>`) — append `/ws/<agent_type>` in tests
/// - the provisioned registry (so tests can register agents)
/// - a `watch::Sender<bool>` to trigger graceful shutdown
/// - the connection thread `JoinHandle` for clean teardown
async fn start_server_with_registry() -> (
    ContainerAsync<Nats>,
    u16,                  // nats_port — for mock agents that need a direct NATS connection
    String,               // base WebSocket URL
    trogon_registry::Registry<trogon_registry::KvStore>,
    watch::Sender<bool>,
    std::thread::JoinHandle<()>,
) {
    let (container, nats_port) = start_nats_with_jetstream().await;

    let nats_client = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("connect to NATS");

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client =
        trogon_nats::jetstream::NatsJetStreamClient::new(js_context.clone());

    let store = trogon_registry::provision(&js_context)
        .await
        .expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    let base_config = make_config(nats_port);

    let registry_ext = Arc::new(RegistryExtension {
        registry: registry.clone(),
        base_config: base_config.clone(),
    });

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

    let conn_thread = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || run_connection_thread(conn_rx, nats_client, js_client, base_config))
        .expect("failed to spawn connection thread");

    let state = UpgradeState {
        conn_tx,
        shutdown_tx: shutdown_tx.clone(),
    };

    let app = axum::Router::new()
        .route(
            "/ws/{agent_type}",
            axum::routing::get(
                upgrade::handle_with_agent_type::<trogon_registry::KvStore>,
            ),
        )
        .layer(axum::extract::Extension(registry_ext))
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

    (container, nats_port, format!("ws://{addr}"), registry, shutdown_tx, conn_thread)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Full E2E: WebSocket client → acp-nats-ws → real NATS → agent subscriber →
/// back to WebSocket client. Asserts that the `initialize` response carries the
/// expected `protocolVersion`.
#[tokio::test]
async fn ws_initialize_with_real_nats_returns_protocol_version() {
    let (_container, nats_port) = start_nats().await;

    // Spin up a NATS subscriber that acts as the agent and replies to initialize.
    let agent_nats = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("agent NATS connect");
    let mut agent_sub = agent_nats.subscribe("acp.agent.initialize").await.unwrap();
    let agent_nats2 = agent_nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                agent_nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let (ws_url, shutdown_tx, conn_thread) = start_server(nats_port).await;

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#;
    ws.send(Message::Text(req.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for initialize response")
        .expect("stream closed before response")
        .unwrap();

    let text = match msg {
        Message::Text(t) => t.to_string(),
        other => panic!("expected Text message, got {other:?}"),
    };

    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["result"]["protocolVersion"],
        serde_json::json!(ProtocolVersion::LATEST),
        "unexpected protocolVersion in response: {text}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// Verifies that a connected WebSocket client observes the connection closing
/// (stream ends or close frame) after the server-side shutdown signal is sent.
#[tokio::test]
async fn ws_connection_closes_cleanly_on_server_shutdown() {
    let (_container, nats_port) = start_nats().await;

    let (ws_url, shutdown_tx, conn_thread) = start_server(nats_port).await;

    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Signal shutdown immediately after the connection is established.
    shutdown_tx.send(true).unwrap();

    // The client should see the stream end (None) or a Close frame.
    // We give the server a moment to propagate the shutdown.
    let outcome = tokio::time::timeout(Duration::from_secs(5), async move {
        loop {
            match ws.next().await {
                None => return,                        // stream ended
                Some(Ok(Message::Close(_))) => return, // close frame received
                Some(Ok(_)) => continue,               // other frames — keep draining
                Some(Err(_)) => return,                // connection error is also acceptable
            }
        }
    })
    .await;

    assert!(
        outcome.is_ok(),
        "timed out waiting for the WebSocket to close after server shutdown"
    );

    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// Full E2E for request_permission: agent sends a NATS request →
/// WS bridge forwards it to the WS client as a JSON-RPC
/// `session/request_permission` call → WS client replies with AllowOnce →
/// WS bridge publishes the raw `RequestPermissionResponse` JSON back to the
/// NATS reply subject → the NATS request resolves with a Selected outcome.
///
/// This verifies the Phase 2 fix: the bridge now uses raw JSON on the NATS
/// wire (matching what `NatsClientProxy::request_permission` sends/expects)
/// instead of the old JSON-RPC envelope format.
#[tokio::test]
async fn ws_request_permission_forwarded_and_replied_via_nats() {
    use agent_client_protocol::{
        PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SelectedPermissionOutcome, ToolCallUpdate, ToolCallUpdateFields,
    };

    let (_container, nats_port) = start_nats().await;
    let (ws_url, shutdown_tx, conn_thread) = start_server(nats_port).await;

    // WS client connects (simulates Zed/editor).
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    // Wait for client::run to establish the NATS wildcard subscription.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Agent side: NATS request simulating trogon-acp-runner calling
    // NatsClientProxy::request_permission. Payload is raw JSON (no JSON-RPC
    // envelope), matching what NatsClientProxy actually sends.
    let agent_nats = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("agent NATS connect");

    const SESSION: &str = "sess-42";
    let subject = format!("acp.session.{SESSION}.client.session.request_permission");
    let perm_request = RequestPermissionRequest::new(
        SESSION,
        ToolCallUpdate::new("call-1", ToolCallUpdateFields::new()),
        vec![
            PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
            PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
        ],
    );
    let request_payload = serde_json::to_vec(&perm_request).unwrap();

    // Spawn the NATS request; it blocks until the WS client replies (or times out).
    let nats_request_handle = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(5),
            agent_nats.request(subject, request_payload.into()),
        )
        .await
    });

    // WS client receives the JSON-RPC session/request_permission call.
    let rpc_msg_str = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => return t.to_string(),
                Some(Ok(_)) => continue,
                other => panic!("unexpected WS message: {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for session/request_permission JSON-RPC message");

    let rpc_msg: serde_json::Value = serde_json::from_str(&rpc_msg_str).unwrap();
    assert_eq!(
        rpc_msg["method"], "session/request_permission",
        "expected session/request_permission method, got: {rpc_msg_str}"
    );
    let rpc_id = rpc_msg["id"].clone();

    // WS client replies with AllowOnce.
    let allow_result = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new("allow"),
    ));
    let rpc_reply = serde_json::json!({
        "jsonrpc": "2.0",
        "id": rpc_id,
        "result": allow_result,
    });
    ws.send(Message::Text(rpc_reply.to_string().into()))
        .await
        .unwrap();

    // The NATS reply should be a raw RequestPermissionResponse JSON (no JSON-RPC
    // envelope), because that's what NatsClientProxy::request_with_timeout parses.
    let nats_outcome = nats_request_handle
        .await
        .expect("task panicked")
        .expect("NATS request timed out")
        .expect("NATS request failed");

    let nats_response: RequestPermissionResponse =
        serde_json::from_slice(&nats_outcome.payload)
            .expect("NATS reply payload should deserialize as raw RequestPermissionResponse");

    assert!(
        matches!(nats_response.outcome, RequestPermissionOutcome::Selected(_)),
        "expected Selected outcome"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// Two WebSocket clients connect simultaneously, each sends an `initialize`
/// request, and each receives its own correctly-correlated response.
#[tokio::test]
async fn multiple_ws_clients_get_independent_responses() {
    let (_container, nats_port) = start_nats().await;

    // Agent subscriber: reply to every initialize request it receives.
    let agent_nats = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("agent NATS connect");
    let mut agent_sub = agent_nats.subscribe("acp.agent.initialize").await.unwrap();
    let agent_nats2 = agent_nats.clone();
    tokio::spawn(async move {
        while let Some(msg) = agent_sub.next().await {
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                let _ = agent_nats2.publish(reply, resp.into()).await;
            }
        }
    });

    let (ws_url, shutdown_tx, conn_thread) = start_server(nats_port).await;

    // Connect two clients.
    let (mut ws1, _) = connect_async(&ws_url).await.unwrap();
    let (mut ws2, _) = connect_async(&ws_url).await.unwrap();

    let req1 = r#"{"jsonrpc":"2.0","id":10,"method":"initialize","params":{"protocolVersion":0}}"#;
    let req2 = r#"{"jsonrpc":"2.0","id":20,"method":"initialize","params":{"protocolVersion":0}}"#;

    ws1.send(Message::Text(req1.into())).await.unwrap();
    ws2.send(Message::Text(req2.into())).await.unwrap();

    // Collect the first response from each client concurrently.
    let (resp1, resp2) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws1.next().await {
                    Some(Ok(Message::Text(t))) => return t.to_string(),
                    Some(Ok(_)) => continue,
                    other => panic!("ws1 unexpected: {other:?}"),
                }
            }
        }),
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws2.next().await {
                    Some(Ok(Message::Text(t))) => return t.to_string(),
                    Some(Ok(_)) => continue,
                    other => panic!("ws2 unexpected: {other:?}"),
                }
            }
        }),
    );

    let text1 = resp1.expect("timed out waiting for ws1 response");
    let text2 = resp2.expect("timed out waiting for ws2 response");

    let val1: serde_json::Value = serde_json::from_str(&text1).unwrap();
    let val2: serde_json::Value = serde_json::from_str(&text2).unwrap();

    // Each client receives a response with its own request id and a protocolVersion.
    assert_eq!(
        val1["id"],
        serde_json::json!(10),
        "wrong id in ws1 response"
    );
    assert_eq!(
        val2["id"],
        serde_json::json!(20),
        "wrong id in ws2 response"
    );
    assert!(
        val1["result"]["protocolVersion"].is_number(),
        "ws1 response missing protocolVersion"
    );
    assert!(
        val2["result"]["protocolVersion"].is_number(),
        "ws2 response missing protocolVersion"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

// ── /ws/:agent_type integration tests ────────────────────────────────────────

/// Full E2E for the agent-type route: client connects to `/ws/claude`, the server
/// looks up "claude" in the real NATS registry, derives prefix `acp.claude`, and
/// forwards the `initialize` request to `acp.claude.agent.initialize`.  The mock
/// agent replies and the client receives a valid `protocolVersion` response.
#[tokio::test]
async fn ws_agent_type_route_uses_registered_prefix() {
    let (_container, nats_port, base_url, registry, shutdown_tx, conn_thread) =
        start_server_with_registry().await;

    // Register "claude" with prefix acp.claude.
    let cap = trogon_registry::AgentCapability {
        agent_type: "claude".to_string(),
        capabilities: vec!["chat".to_string()],
        nats_subject: "acp.claude.agent.>".to_string(),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": "acp.claude" }),
    };
    registry.register(&cap).await.expect("register claude");

    // Mock agent: subscribe on the claude-prefixed initialize subject.
    let agent_nats = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("agent NATS connect");
    let mut agent_sub = agent_nats
        .subscribe("acp.claude.agent.initialize")
        .await
        .unwrap();
    let agent_nats2 = agent_nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                agent_nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let ws_url = format!("{base_url}/ws/claude");
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#;
    ws.send(Message::Text(req.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for initialize response")
        .expect("stream closed before response")
        .unwrap();

    let text = match msg {
        Message::Text(t) => t.to_string(),
        other => panic!("expected Text message, got {other:?}"),
    };

    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["result"]["protocolVersion"],
        serde_json::json!(ProtocolVersion::LATEST),
        "unexpected response: {text}"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

/// Connecting to `/ws/<unregistered>` returns HTTP 404 — the registry lookup
/// fails before the WebSocket upgrade is completed.
#[tokio::test]
async fn ws_agent_type_route_returns_404_for_unregistered_agent() {
    let (_container, _nats_port, base_url, _registry, shutdown_tx, conn_thread) =
        start_server_with_registry().await;

    // No agent registered — connect attempt must be rejected with 404.
    let ws_url = format!("{base_url}/ws/ghost");
    let result = connect_async(&ws_url).await;

    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 404, "expected 404, got {}", resp.status());
        }
        Ok(_) => panic!("expected connection to be rejected with 404"),
        Err(e) => panic!("unexpected error: {e}"),
    }

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}

// ── Runner isolation test ─────────────────────────────────────────────────────

/// Core isolation invariant: two agents registered with distinct prefixes
/// (`acp.xai` and `acp.claude`) must each only receive messages from their own
/// `/ws/<agent_type>` path.  A client connecting to `/ws/xai` must deliver
/// messages to the xai mock agent and never to the claude mock agent.
#[tokio::test]
async fn runner_isolation_messages_only_reach_registered_runner() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let (_container, nats_port, base_url, registry, shutdown_tx, conn_thread) =
        start_server_with_registry().await;

    // Register two agents with distinct prefixes.
    registry
        .register(&trogon_registry::AgentCapability {
            agent_type: "xai".to_string(),
            capabilities: vec!["chat".to_string()],
            nats_subject: "acp.xai.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::json!({ "acp_prefix": "acp.xai" }),
        })
        .await
        .expect("register xai");
    registry
        .register(&trogon_registry::AgentCapability {
            agent_type: "claude".to_string(),
            capabilities: vec!["chat".to_string()],
            nats_subject: "acp.claude.agent.>".to_string(),
            current_load: 0,
            metadata: serde_json::json!({ "acp_prefix": "acp.claude" }),
        })
        .await
        .expect("register claude");

    // xai mock agent: subscribes, replies, and records receipt.
    let xai_received = Arc::new(AtomicBool::new(false));
    let xai_nats = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("xai agent NATS connect");
    let mut xai_sub = xai_nats.subscribe("acp.xai.agent.initialize").await.unwrap();
    let xai_received2 = xai_received.clone();
    let xai_nats2 = xai_nats.clone();
    tokio::spawn(async move {
        while let Some(msg) = xai_sub.next().await {
            xai_received2.store(true, Ordering::Relaxed);
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                xai_nats2.publish(reply, resp.into()).await.ok();
            }
        }
    });

    // claude mock agent: subscribes and records any unexpected receipt (no reply needed).
    let claude_received = Arc::new(AtomicBool::new(false));
    let claude_nats = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("claude agent NATS connect");
    let mut claude_sub = claude_nats
        .subscribe("acp.claude.agent.initialize")
        .await
        .unwrap();
    let claude_received2 = claude_received.clone();
    tokio::spawn(async move {
        while let Some(_msg) = claude_sub.next().await {
            claude_received2.store(true, Ordering::Relaxed);
        }
    });

    // Client connects to /ws/xai and sends initialize.
    let ws_url = format!("{base_url}/ws/xai");
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#;
    ws.send(Message::Text(req.into())).await.unwrap();

    let _msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for xai agent response")
        .expect("stream closed before response")
        .unwrap();

    assert!(
        xai_received.load(Ordering::Relaxed),
        "xai agent must have received the initialize request routed via acp.xai.*"
    );

    // Give any stray messages time to arrive before asserting claude's silence.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !claude_received.load(Ordering::Relaxed),
        "claude agent must NOT receive messages routed to /ws/xai — channel isolation broken"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
}
