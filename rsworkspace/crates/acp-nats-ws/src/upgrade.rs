use axum::extract::State;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::response::Response;
use tokio::sync::{mpsc, watch};
use tracing::error;

pub struct ConnectionRequest {
    pub socket: WebSocket,
    pub shutdown_rx: watch::Receiver<bool>,
}

#[derive(Clone)]
pub struct UpgradeState {
    pub conn_tx: mpsc::UnboundedSender<ConnectionRequest>,
    pub shutdown_tx: watch::Sender<bool>,
}

pub async fn handle(ws: WebSocketUpgrade, State(state): State<UpgradeState>) -> Response {
    let shutdown_rx = state.shutdown_tx.subscribe();
    ws.on_upgrade(move |socket| async move {
        if state
            .conn_tx
            .send(ConnectionRequest {
                socket,
                shutdown_rx,
            })
            .is_err()
        {
            error!("Connection thread is gone; dropping WebSocket");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use futures_util::StreamExt;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::connect_async;

    #[tokio::test]
    async fn upgrade_handle_conn_tx_gone_does_not_panic() {
        // Drop conn_rx immediately → send() will fail → error! path is exercised
        let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();
        let (shutdown_tx, _) = watch::channel(false);
        drop(conn_rx);

        let state = UpgradeState {
            conn_tx,
            shutdown_tx,
        };

        let app = Router::new()
            .route("/ws", axum::routing::get(handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // The upgrade handshake succeeds (101), but then conn_tx.send fails
        // and the socket is dropped — the client sees the connection close.
        let url = format!("ws://{}/ws", addr);
        if let Ok((mut ws, _)) = connect_async(url).await {
            // Socket is dropped on server side → client sees close or None
            let _ = tokio::time::timeout(Duration::from_secs(1), ws.next()).await;
        }

        server.abort();
    }
}
