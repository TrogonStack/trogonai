use acp_nats::{StdJsonSerialize, agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::{AgentSideConnection, SessionNotification};
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
#[cfg_attr(coverage, coverage(off))]
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
    let (notification_tx, notification_rx) = tokio::sync::mpsc::channel::<SessionNotification>(64);
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
        SystemClock,
        &meter,
        config,
        notification_tx,
    ));

    let (connection, io_task) =
        AgentSideConnection::new(bridge.clone(), outgoing, incoming, |fut| {
            tokio::task::spawn_local(fut);
        });

    let connection = Rc::new(connection);

    spawn_notification_forwarder(connection.clone(), notification_rx);

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

#[cfg_attr(coverage, coverage(off))]
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

#[cfg_attr(coverage, coverage(off))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::extract::ws::WebSocketUpgrade;
    use axum::response::Response;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    #[derive(Clone)]
    struct EchoState;

    async fn echo_handler(ws: WebSocketUpgrade, State(_): State<EchoState>) -> Response {
        ws.on_upgrade(|socket| async move {
            let (ws_sender, ws_receiver) = socket.split();
            let (duplex_write, duplex_read) = tokio::io::duplex(DUPLEX_BUFFER_SIZE);
            let recv = run_recv_pump(ws_receiver, duplex_write);
            let send = run_send_pump(ws_sender, duplex_read);
            tokio::join!(recv, send);
        })
    }

    async fn start_echo_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route("/ws", axum::routing::get(echo_handler))
            .with_state(EchoState);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("ws://{}/ws", addr)
    }

    #[tokio::test]
    async fn multiple_messages_round_trip() {
        let url = start_echo_server().await;
        let (mut ws, _) = connect_async(&url).await.unwrap();

        let messages = vec!["alpha", "beta", "gamma"];
        for msg in &messages {
            ws.send(TungsteniteMessage::Text((*msg).into()))
                .await
                .unwrap();
        }

        for expected in &messages {
            let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
                .await
                .expect("timeout")
                .expect("stream ended")
                .unwrap();
            match msg {
                TungsteniteMessage::Text(t) => assert_eq!(t, *expected),
                other => panic!("expected Text('{expected}'), got {other:?}"),
            }
        }
    }
}
