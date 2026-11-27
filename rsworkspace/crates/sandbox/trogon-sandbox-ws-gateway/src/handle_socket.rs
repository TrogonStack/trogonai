use std::sync::Arc;

use axum::extract::ws::WebSocket;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::connection::Connection;
use crate::constants::OUTBOUND_QUEUE_CAPACITY;
use crate::pending_requests::PendingRequests;
use crate::{PreviewState, SandboxBackend, SessionRecord, SessionStore, TerminalSessions, ToolContext, WsFrame};

/// Serves one authenticated socket until either side closes it.
pub async fn handle_socket(
    socket: WebSocket,
    session: SessionRecord,
    backend: Arc<dyn SandboxBackend>,
    sessions: Arc<dyn SessionStore>,
    preview: Option<Arc<PreviewState>>,
) {
    let (ws_tx, mut ws_rx) = socket.split();
    let (events, mut event_rx) = mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let mut connection = Connection {
        ws_tx,
        ctx: ToolContext {
            session,
            backend,
            sessions,
            preview,
            events,
            dashboard_cancel: CancellationToken::new(),
        },
        terminals: TerminalSessions::default(),
        requests: JoinSet::new(),
        pending: PendingRequests::default(),
    };

    loop {
        let alive = tokio::select! {
            completed = connection.requests.join_next(), if !connection.requests.is_empty() => {
                let Some(Ok((id, control, frame))) = completed else { break };
                connection.pending.finish(&id, control);
                connection.send(&frame).await
            }
            Some((stream_id, event)) = event_rx.recv() => connection.send(&WsFrame::event(stream_id, event)).await,
            message = ws_rx.next() => {
                let Some(Ok(message)) = message else { break };
                connection.on_message(message).await
            }
        };
        if !alive {
            break;
        }
    }

    connection.close().await;
}
