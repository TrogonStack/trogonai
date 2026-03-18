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
