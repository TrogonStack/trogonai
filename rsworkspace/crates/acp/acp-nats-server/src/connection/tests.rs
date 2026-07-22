use super::*;
use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::response::Response;
use std::error::Error;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

use crate::constants::ACP_ENDPOINT;

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
        .route(ACP_ENDPOINT, axum::routing::get(echo_handler))
        .with_state(EchoState);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("ws://{}{}", addr, ACP_ENDPOINT)
}

#[tokio::test]
async fn shutdown_error_wraps_join_error_source() {
    let handle = tokio::spawn(std::future::pending::<()>());
    handle.abort();
    let source = handle.await.unwrap_err();
    assert!(source.is_cancelled());

    let error = ConnectionShutdownError::ClientTask { source };

    assert!(matches!(error, ConnectionShutdownError::ClientTask { .. }));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn multiple_messages_round_trip() {
    let url = start_echo_server().await;
    let (mut ws, _) = connect_async(&url).await.unwrap();

    let messages = vec!["alpha", "beta", "gamma"];
    for msg in &messages {
        ws.send(TungsteniteMessage::Text((*msg).into())).await.unwrap();
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

#[tokio::test]
async fn binary_messages_are_ignored() {
    let url = start_echo_server().await;
    let (mut ws, _) = connect_async(&url).await.unwrap();

    ws.send(TungsteniteMessage::Binary(bytes::Bytes::from_static(b"ignored")))
        .await
        .unwrap();
    ws.send(TungsteniteMessage::Text("kept".into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .unwrap();

    match msg {
        TungsteniteMessage::Text(text) => assert_eq!(text, "kept"),
        other => panic!("expected text frame, got {other:?}"),
    }
}
