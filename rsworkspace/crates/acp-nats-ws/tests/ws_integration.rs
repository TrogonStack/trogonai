//! Integration tests for acp-nats-ws with a real NATS server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p acp-nats-ws --test ws_integration

use std::time::Duration;

use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
use acp_nats_ws::upgrade::{ConnectionRequest, UpgradeState};
use acp_nats_ws::{THREAD_NAME, run_connection_thread, upgrade};
use agent_client_protocol::{InitializeResponse, ProtocolVersion};
use futures_util::{SinkExt, StreamExt};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
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

/// Starts the acp-nats-ws server backed by real NATS.
///
/// Returns:
/// - the WebSocket URL (`ws://127.0.0.1:<port>/ws`)
/// - a `watch::Sender<bool>` to trigger graceful shutdown
/// - the connection thread `JoinHandle` for clean teardown
async fn start_server(
    nats_port: u16,
) -> (String, watch::Sender<bool>, std::thread::JoinHandle<()>) {
    let nats_client = async_nats::connect(format!("127.0.0.1:{nats_port}"))
        .await
        .expect("connect to NATS");

    let config = make_config(nats_port);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

    let conn_thread = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || run_connection_thread(conn_rx, nats_client, config))
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
