use acp_nats::{StdJsonSerialize, agent::Bridge, client};
use agent_client_protocol::AgentSideConnection;
use axum::extract::ws::{Message, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use std::rc::Rc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tracing::{error, info, warn};
use trogon_std::time::SystemClock;

const DUPLEX_BUFFER_SIZE: usize = 64 * 1024;

/// Handles a single WebSocket connection by bridging it to NATS via ACP.
pub async fn handle<N>(
    socket: WebSocket,
    nats_client: N,
    config: acp_nats::Config,
    mut shutdown_rx: watch::Receiver<bool>,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + 'static,
{
    let (ws_sender, ws_receiver) = socket.split();

    let (agent_write, ws_send_read) = tokio::io::duplex(DUPLEX_BUFFER_SIZE);
    let (ws_recv_write, agent_read) = tokio::io::duplex(DUPLEX_BUFFER_SIZE);

    let incoming = async_compat::Compat::new(agent_read);
    let outgoing = async_compat::Compat::new(agent_write);

    let meter = acp_telemetry::meter("acp-nats-ws");
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

    let recv_pump = tokio::task::spawn_local(run_recv_pump(ws_receiver, ws_recv_write));
    let send_pump = tokio::task::spawn_local(run_send_pump(ws_sender, ws_send_read));

    let mut client_task = tokio::task::spawn_local(client::run(
        nats_client,
        connection.clone(),
        bridge.clone(),
        StdJsonSerialize,
    ));

    let mut io_task = tokio::task::spawn_local(io_task);

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
        result = &mut io_task => {
            let res = result
                .map_err(|e| format!("io task spawn error: {e}"))
                .and_then(|r| r.map_err(|e| format!("io task error: {e}")));

            match res {
                Ok(_) => {
                    info!("IO task completed (connection closed)");
                    Ok(())
                }
                Err(e) => {
                    error!(error = e, "IO task ended with error");
                    Err(e)
                }
            }
        }
        _ = shutdown_rx.wait_for(|&v| v) => {
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
    if !io_task.is_finished() {
        io_task.abort();
        let _ = io_task.await;
    }

    match shutdown_result {
        Ok(()) => info!("WebSocket connection closed cleanly"),
        Err(e) => warn!(error = e, "WebSocket connection closed with error"),
    }
}

async fn run_recv_pump(
    mut ws_receiver: SplitStream<WebSocket>,
    mut ws_recv_write: tokio::io::DuplexStream,
) {
    while let Some(Ok(msg)) = ws_receiver.next().await {
        let bytes = match msg {
            Message::Text(t) => bytes::Bytes::from(t),
            Message::Binary(b) => b,
            Message::Close(_) => break,
            _ => continue,
        };

        match std::str::from_utf8(&bytes) {
            Ok(text) => {
                let line = text.trim_end_matches(['\r', '\n']);
                if line.is_empty() {
                    continue;
                }

                if ws_recv_write.write_all(line.as_bytes()).await.is_err() {
                    break;
                }

                if ws_recv_write.write_all(b"\n").await.is_err() {
                    break;
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Received non-UTF-8 WebSocket message, dropping frame"
                );
            }
        }
    }
}

async fn run_send_pump(
    mut ws_sender: SplitSink<WebSocket, Message>,
    ws_send_read: tokio::io::DuplexStream,
) {
    let mut reader = tokio::io::BufReader::new(ws_send_read);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\r', '\n']);
                if trimmed.is_empty() {
                    continue;
                }
                if ws_sender.send(Message::Text(trimmed.into())).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = ws_sender.close().await;
}
