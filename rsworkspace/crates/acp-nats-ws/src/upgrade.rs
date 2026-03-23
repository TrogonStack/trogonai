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
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::connect_async;

    #[tokio::test]
    async fn handle_sends_connection_request_through_channel() {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

        let state = UpgradeState {
            conn_tx,
            shutdown_tx: shutdown_tx.clone(),
        };

        let app = axum::Router::new()
            .route("/ws", axum::routing::get(handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("ws://{}/ws", addr);
        let (_ws, _) = connect_async(&url).await.unwrap();

        let req = tokio::time::timeout(Duration::from_secs(2), conn_rx.recv())
            .await
            .expect("timeout waiting for ConnectionRequest")
            .expect("channel closed");

        assert!(!*req.shutdown_rx.borrow());
    }

    #[tokio::test]
    async fn handle_logs_error_when_conn_rx_dropped() {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

        let state = UpgradeState {
            conn_tx,
            shutdown_tx: shutdown_tx.clone(),
        };

        let app = axum::Router::new()
            .route("/ws", axum::routing::get(handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        drop(conn_rx);

        let url = format!("ws://{}/ws", addr);
        let (_ws, _) = connect_async(&url).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
