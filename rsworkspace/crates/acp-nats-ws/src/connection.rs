use acp_nats::{StdJsonSerialize, agent::Bridge, client};
use agent_client_protocol::AgentSideConnection;
use async_nats::Client as NatsAsyncClient;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::rc::Rc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use trogon_std::time::SystemClock;

const DUPLEX_BUFFER_SIZE: usize = 64 * 1024;

/// Handles a single WebSocket connection by bridging it to NATS via ACP.
///
/// Each connection gets its own `Bridge`, `AgentSideConnection`, and NATS
/// client dispatcher -- the same wiring as `run_bridge` in `acp-nats-stdio`,
/// but reading/writing through WebSocket frames instead of stdin/stdout.
///
/// ## Message framing
///
/// ACP and MCP both use newline-delimited JSON-RPC over byte streams (stdio).
/// Over WebSocket, the convention (aligned with MCP SEP-1288) is **one WS
/// text message = one JSON-RPC message**, with no trailing newline in the
/// frame payload.
///
/// The duplex-pipe adapter bridges these two worlds:
///
/// - **Recv pump**: appends `\n` after each incoming WS text message so the
///   `AgentSideConnection` reader sees newline-delimited JSON-RPC.
/// - **Send pump**: reads newline-delimited lines from the outgoing pipe and
///   sends each as a separate WS text frame (without the `\n`).
pub async fn handle_connection(
    socket: WebSocket,
    nats_client: NatsAsyncClient,
    config: acp_nats::Config,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Duplex pipes bridge the message-based WebSocket to the byte-stream
    // interface that AgentSideConnection expects (futures::io::AsyncRead/Write).
    let (agent_write, ws_send_read) = tokio::io::duplex(DUPLEX_BUFFER_SIZE);
    let (mut ws_recv_write, agent_read) = tokio::io::duplex(DUPLEX_BUFFER_SIZE);

    let incoming = async_compat::Compat::new(agent_read);
    let outgoing = async_compat::Compat::new(agent_write);

    let meter = opentelemetry::global::meter("acp-nats-ws");
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
        SystemClock,
        &meter,
        config,
    ));

    let (connection, io_task) =
        AgentSideConnection::new(bridge.clone(), outgoing, incoming, |fut| {
            tokio::task::spawn_local(fut);
        });

    let connection = Rc::new(connection);

    // WS recv pump: each incoming WS text message is one JSON-RPC message.
    // Append `\n` so the byte-stream reader inside AgentSideConnection can
    // parse newline-delimited messages.
    let recv_pump = tokio::task::spawn_local(async move {
        while let Some(result) = ws_receiver.next().await {
            match result {
                Ok(Message::Text(text)) => {
                    if ws_recv_write.write_all(text.as_bytes()).await.is_err()
                        || ws_recv_write.write_all(b"\n").await.is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Binary(data)) => {
                    if ws_recv_write.write_all(&data).await.is_err()
                        || ws_recv_write.write_all(b"\n").await.is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "WebSocket receive error");
                    break;
                }
            }
        }
        drop(ws_recv_write);
    });

    // WS send pump: read newline-delimited JSON-RPC messages from the pipe
    // and send each as a separate WS text frame (without the trailing `\n`).
    let send_pump = tokio::task::spawn_local(async move {
        let mut reader = BufReader::new(ws_send_read);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim_end_matches('\n');
                    if trimmed.is_empty() {
                        continue;
                    }
                    if ws_sender
                        .send(Message::Text(trimmed.to_owned().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = ws_sender.close().await;
    });

    // Client task: dispatches NATS client subject messages.
    let client_connection = connection.clone();
    let bridge_for_client = bridge.clone();
    let mut client_task = tokio::task::spawn_local(async move {
        client::run(
            nats_client,
            client_connection,
            bridge_for_client,
            StdJsonSerialize,
        )
        .await;
    });

    info!("WebSocket connection established, ACP bridge running");

    let shutdown_result = tokio::select! {
        result = &mut client_task => {
            match result {
                Ok(()) => {
                    info!("Client task completed");
                    Ok(())
                }
                Err(e) => {
                    error!(error = %e, "Client task ended with error");
                    Err(format!("client task error: {e}"))
                }
            }
        }
        result = io_task => {
            match result {
                Err(e) => {
                    error!(error = %e, "IO task error");
                    Err(format!("io task error: {e}"))
                }
                Ok(()) => {
                    info!("IO task completed (connection closed)");
                    Ok(())
                }
            }
        }
        _ = shutdown_rx.recv() => {
            info!("Connection shutting down (server shutdown)");
            Ok(())
        }
    };

    recv_pump.abort();
    send_pump.abort();

    if !client_task.is_finished() {
        client_task.abort();
        let _ = client_task.await;
    }

    match shutdown_result {
        Ok(()) => info!("WebSocket connection closed cleanly"),
        Err(e) => warn!(error = e, "WebSocket connection closed with error"),
    }
}
