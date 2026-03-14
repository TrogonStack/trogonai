use acp_nats::{StdJsonSerialize, agent::Bridge, client};
use agent_client_protocol::AgentSideConnection;
use async_nats::Client as NatsAsyncClient;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::rc::Rc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use trogon_std::time::SystemClock;

const DUPLEX_BUFFER_SIZE: usize = 64 * 1024;

/// Handles a single WebSocket connection by bridging it to NATS via ACP.
///
/// Each connection gets its own `Bridge`, `AgentSideConnection`, and NATS
/// client dispatcher -- the same wiring as `run_bridge` in `acp-nats-stdio`,
/// but reading/writing through WebSocket frames instead of stdin/stdout.
pub async fn handle_connection(
    socket: WebSocket,
    nats_client: NatsAsyncClient,
    config: acp_nats::Config,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Duplex pipes bridge the message-based WebSocket to the byte-stream
    // interface that AgentSideConnection expects (futures::io::AsyncRead/Write).
    let (agent_write, mut ws_send_read) = tokio::io::duplex(DUPLEX_BUFFER_SIZE);
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

    // WS recv pump: read WebSocket text/binary frames, write raw bytes into the
    // pipe that AgentSideConnection reads from.
    let recv_pump = tokio::task::spawn_local(async move {
        while let Some(result) = ws_receiver.next().await {
            match result {
                Ok(Message::Text(text)) => {
                    if ws_recv_write.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Binary(data)) => {
                    if ws_recv_write.write_all(&data).await.is_err() {
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

    // WS send pump: read bytes from the pipe that AgentSideConnection writes to,
    // send as WebSocket text frames.
    let send_pump = tokio::task::spawn_local(async move {
        let mut buf = vec![0u8; DUPLEX_BUFFER_SIZE];
        loop {
            match ws_send_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if ws_sender.send(Message::Text(text.into())).await.is_err() {
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
