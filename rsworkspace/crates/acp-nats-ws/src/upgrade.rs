use crate::acp_connection_id::AcpConnectionId;
use crate::constants::ACP_CONNECTION_ID_HEADER;
use axum::extract::State;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::http::HeaderValue;
use axum::response::Response;
use tokio::sync::{mpsc, watch};
use tracing::error;

pub struct ConnectionRequest {
    pub connection_id: AcpConnectionId,
    pub socket: WebSocket,
    pub shutdown_rx: watch::Receiver<bool>,
}

#[derive(Clone)]
pub struct UpgradeState {
    pub conn_tx: mpsc::UnboundedSender<ConnectionRequest>,
    pub shutdown_tx: watch::Sender<bool>,
}

pub async fn handle(ws: WebSocketUpgrade, State(state): State<UpgradeState>) -> Response {
    let connection_id = AcpConnectionId::new();
    let response_header = HeaderValue::from_str(&connection_id.to_string())
        .expect("generated ACP connection id must be a valid header value");
    let shutdown_rx = state.shutdown_tx.subscribe();
    let mut response = ws.on_upgrade(move |socket| async move {
        if state
            .conn_tx
            .send(ConnectionRequest {
                connection_id,
                socket,
                shutdown_rx,
            })
            .is_err()
        {
            error!("Connection thread is gone; dropping WebSocket");
        }
    });
    response
        .headers_mut()
        .insert(ACP_CONNECTION_ID_HEADER, response_header);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{ACP_CONNECTION_ID_HEADER, ACP_ENDPOINT};
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
            .route(ACP_ENDPOINT, axum::routing::get(handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("ws://{}{}", addr, ACP_ENDPOINT);
        let (_ws, response) = connect_async(&url).await.unwrap();
        let connection_id = response
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .expect("upgrade response should include Acp-Connection-Id")
            .to_str()
            .unwrap()
            .to_string();

        let req = tokio::time::timeout(Duration::from_secs(2), conn_rx.recv())
            .await
            .expect("timeout waiting for ConnectionRequest")
            .expect("channel closed");

        assert!(!*req.shutdown_rx.borrow());
        assert_eq!(req.connection_id.to_string(), connection_id);
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
            .route(ACP_ENDPOINT, axum::routing::get(handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        drop(conn_rx);

        let url = format!("ws://{}{}", addr, ACP_ENDPOINT);
        let (_ws, _) = connect_async(&url).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
