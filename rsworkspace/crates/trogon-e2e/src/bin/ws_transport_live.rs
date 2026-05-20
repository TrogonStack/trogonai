//! Live end-to-end test for acp-nats-ws WebSocket transport.
//!
//! Tests the full pipeline: WS client → acp-nats-ws bridge → NATS → TrogonAgent
//! (MockAgentRunner, no LLM credentials) → NATS → bridge → WS client.
//!
//! Requirements:
//!   • NATS running on nats://localhost:4222  (JetStream required)
//!   • No external credentials — MockAgentRunner handles all LLM calls
//!
//! Run:
//!   cargo run -p trogon-e2e --bin ws_transport_live

use std::sync::Arc;
use std::time::Duration;

use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
use acp_nats_agent::AgentSideNatsConnection;
use acp_nats_ws::upgrade::{ConnectionRequest, UpgradeState};
use acp_nats_ws::{run_connection_thread, upgrade};
use async_nats::jetstream;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc, watch};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionNotifier, NatsSessionStore,
    TrogonAgent, agent_runner::mock::MockAgentRunner,
};
use trogon_nats::jetstream::NatsJetStreamClient;
use uuid::Uuid;

// ── Output helpers ─────────────────────────────────────────────────────────────

fn ok(label: &str) {
    println!("  \x1b[32m✓\x1b[0m  {label}");
}

fn ko(label: &str, reason: &str) {
    println!("  \x1b[31m✗\x1b[0m  {label}");
    println!("       {reason}");
}

fn uid() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

// ── Type alias ─────────────────────────────────────────────────────────────────

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

// ── WS message helpers ─────────────────────────────────────────────────────────

/// Read next Text frame, skipping non-Text frames.
async fn next_text(ws: &mut WsStream) -> String {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return t.to_string(),
            Some(Ok(_)) => continue,
            other => panic!("unexpected ws message: {other:?}"),
        }
    }
}

/// Read text frames until finding one whose `"id"` matches `expected_id`.
async fn next_with_id(ws: &mut WsStream, expected_id: i64) -> serde_json::Value {
    for _ in 0..50usize {
        let text = tokio::time::timeout(Duration::from_secs(8), next_text(ws))
            .await
            .expect("timeout waiting for WS frame");
        let val: serde_json::Value = serde_json::from_str(&text).unwrap();
        if val["id"].as_i64() == Some(expected_id) {
            return val;
        }
    }
    panic!("did not receive response with id={expected_id} after 50 frames");
}

// ── Infrastructure setup ───────────────────────────────────────────────────────

fn make_acp_config() -> Config {
    Config::new(
        AcpPrefix::new("acp").unwrap(),
        NatsConfig {
            servers: vec!["127.0.0.1:4222".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_secs(5))
}

/// Provision all JetStream streams required by the ACP protocol.
async fn setup_acp_streams(js: &jetstream::Context) {
    let prefix = AcpPrefix::new("acp").unwrap();
    for config in acp_nats::jetstream::streams::all_configs(&prefix) {
        js.get_or_create_stream(config).await.unwrap();
    }
}

/// Start a TrogonAgent with MockAgentRunner on its own OS thread + LocalSet.
fn start_mock_agent(nats: async_nats::Client) {
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

        let gateway_config = Arc::new(RwLock::new(None::<GatewayConfig>));
        let notifier = NatsSessionNotifier::new(nats.clone());
        let agent = TrogonAgent::new(
            notifier,
            store,
            MockAgentRunner::new("test-model"),
            "acp",
            "test-model",
            None,
            None,
            gateway_config,
        );
        let prefix = AcpPrefix::new("acp").unwrap();
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
}

/// Start the acp-nats-ws bridge server; returns (ws_url, shutdown_tx).
async fn start_ws_server(nats: async_nats::Client, js: jetstream::Context) -> (String, watch::Sender<bool>) {
    let js_client = NatsJetStreamClient::new(js);
    let config = make_acp_config();
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

    std::thread::Builder::new()
        .name("acp-nats-ws".into())
        .spawn(move || run_connection_thread(conn_rx, nats.clone(), js_client, config))
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

    (format!("ws://{addr}/ws"), shutdown_tx)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

async fn test_ws_initialize(ws_url: &str) -> bool {
    const LABEL: &str = "WS transport — initialize returns protocolVersion + capabilities";

    let result: Result<(), String> = async {
        let (mut ws, _) = connect_async(ws_url).await.map_err(|e| e.to_string())?;

        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#;
        ws.send(Message::Text(req.into())).await.map_err(|e| e.to_string())?;

        let text = tokio::time::timeout(Duration::from_secs(8), next_text(&mut ws))
            .await
            .map_err(|_| "timed out waiting for initialize response".to_string())?;

        let val: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}"))?;

        if val["id"].as_i64() != Some(1) {
            return Err(format!("expected id=1, got: {}", val["id"]));
        }
        if val["result"]["protocolVersion"].is_null() {
            return Err(format!("protocolVersion missing: {text}"));
        }
        ws.close(None).await.ok();
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_ws_new_session(ws_url: &str) -> bool {
    const LABEL: &str = "WS transport — new_session returns sessionId";

    let result: Result<(), String> = async {
        let (mut ws, _) = connect_async(ws_url).await.map_err(|e| e.to_string())?;

        // initialize first (ACP protocol requires it)
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#;
        ws.send(Message::Text(init.into())).await.map_err(|e| e.to_string())?;
        next_with_id(&mut ws, 1).await;

        let cwd = format!("/tmp/ws-test-{}", uid());
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": cwd, "mcpServers": [] }
        });
        ws.send(Message::Text(req.to_string().into())).await.map_err(|e| e.to_string())?;

        let resp = next_with_id(&mut ws, 2).await;
        let sid = resp["result"]["sessionId"]
            .as_str()
            .ok_or_else(|| format!("sessionId missing from new_session response: {resp}"))?
            .to_string();

        if sid.is_empty() {
            return Err("sessionId is empty".to_string());
        }
        ws.close(None).await.ok();
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

async fn test_ws_multiple_connections(ws_url: &str) -> bool {
    const LABEL: &str = "WS transport — two concurrent connections both succeed independently";

    let result: Result<(), String> = async {
        let url1 = ws_url.to_string();
        let url2 = ws_url.to_string();

        let (mut ws1, _) = connect_async(&url1).await.map_err(|e| e.to_string())?;
        let (mut ws2, _) = connect_async(&url2).await.map_err(|e| e.to_string())?;

        let req1 = r#"{"jsonrpc":"2.0","id":10,"method":"initialize","params":{"protocolVersion":0}}"#;
        let req2 = r#"{"jsonrpc":"2.0","id":20,"method":"initialize","params":{"protocolVersion":0}}"#;

        ws1.send(Message::Text(req1.into())).await.map_err(|e| e.to_string())?;
        ws2.send(Message::Text(req2.into())).await.map_err(|e| e.to_string())?;

        let r1 = next_with_id(&mut ws1, 10).await;
        let r2 = next_with_id(&mut ws2, 20).await;

        if r1["result"]["protocolVersion"].is_null() {
            return Err(format!("conn1 missing protocolVersion: {r1}"));
        }
        if r2["result"]["protocolVersion"].is_null() {
            return Err(format!("conn2 missing protocolVersion: {r2}"));
        }

        ws1.close(None).await.ok();
        ws2.close(None).await.ok();
        Ok(())
    }.await;

    match result { Ok(()) => { ok(LABEL); true } Err(e) => { ko(LABEL, &e); false } }
}

// ══════════════════════════════════════════════════════════════════════════════
// Entry point
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() {
    println!();
    println!("══════════════════════════════════════════════════════════");
    println!(" acp-nats-ws — WebSocket transport live test");
    println!("  NATS: nats://localhost:4222  (JetStream required)");
    println!("  LLM:  MockAgentRunner — no credentials needed");
    println!("══════════════════════════════════════════════════════════");
    println!();

    let nats = async_nats::connect("nats://localhost:4222")
        .await
        .expect("NATS must be running on localhost:4222 — start with: nats-server -js");
    let js = jetstream::new(nats.clone());

    setup_acp_streams(&js).await;
    start_mock_agent(nats.clone());
    tokio::time::sleep(Duration::from_millis(300)).await;

    let (ws_url, _shutdown_tx) = start_ws_server(nats.clone(), js.clone()).await;

    println!("WebSocket bridge: {ws_url}");
    println!();

    let r1 = test_ws_initialize(&ws_url).await;
    let r2 = test_ws_new_session(&ws_url).await;
    let r3 = test_ws_multiple_connections(&ws_url).await;

    let results = [r1, r2, r3];
    let passed = results.iter().filter(|&&r| r).count();
    let total = results.len();

    println!();
    println!("══════════════════════════════════════════════════════════");
    if passed == total {
        println!(" \x1b[32mAll {total} tests passed\x1b[0m");
    } else {
        println!(" \x1b[31m{passed}/{total} tests passed\x1b[0m");
    }
    println!("══════════════════════════════════════════════════════════");
    println!();

    if passed < total {
        std::process::exit(1);
    }
}
