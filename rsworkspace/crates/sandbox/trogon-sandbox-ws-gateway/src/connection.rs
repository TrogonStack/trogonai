use axum::extract::ws::{Message, WebSocket};
use futures::SinkExt;
use futures::stream::SplitSink;
use tokio::task::JoinSet;
use tracing::debug;

use crate::constants::{CONCURRENT_TOOLS, CONTROL_TOOLS};
use crate::pending_requests::PendingRequests;
use crate::tool_dispatch::{run_inline, run_tool};
use crate::{TerminalSessions, ToolContext, WsFrame};

/// One socket's state: what it may call, what it has in flight and the terminals it opened.
pub(crate) struct Connection {
    pub ws_tx: SplitSink<WebSocket, Message>,
    pub ctx: ToolContext,
    pub terminals: TerminalSessions,
    /// Requests running beside the socket loop, as `(id, control, response)`.
    pub requests: JoinSet<(String, bool, WsFrame)>,
    pub pending: PendingRequests,
}

impl Connection {
    /// Sends one frame. `false` when the peer is gone.
    pub(crate) async fn send(&mut self, frame: &WsFrame) -> bool {
        let text = serde_json::to_string(frame);
        let Ok(text) = text else { return false };
        self.ws_tx.send(Message::Text(text.into())).await.is_ok()
    }

    /// Handles one incoming message. `false` ends the connection.
    pub(crate) async fn on_message(&mut self, message: Message) -> bool {
        match message {
            Message::Text(text) => self.on_text(text.as_str()).await,
            Message::Close(_) => false,
            Message::Ping(payload) => self.ws_tx.send(Message::Pong(payload)).await.is_ok(),
            Message::Binary(_) | Message::Pong(_) => true,
        }
    }

    async fn on_text(&mut self, text: &str) -> bool {
        let (id, tool, payload) = match serde_json::from_str::<WsFrame>(text) {
            Ok(WsFrame::Request { id, tool, payload }) => (id, tool, payload),
            Ok(_) => {
                let frame = WsFrame::response_err("unknown", "invalid_frame", "expected request frame");
                return self.send(&frame).await;
            }
            Err(error) => {
                let frame = WsFrame::response_err("unknown", "invalid_frame", error.to_string());
                return self.send(&frame).await;
            }
        };
        if let Err(refusal) = self.pending.check_id(&id) {
            return self.send(&refusal.into_frame(id)).await;
        }
        let control = CONTROL_TOOLS.contains(&tool.as_str());
        if control || CONCURRENT_TOOLS.contains(&tool.as_str()) {
            if let Err(refusal) = self.pending.admit(&id, control) {
                return self.send(&refusal.into_frame(id)).await;
            }
            let ctx = self.ctx.clone();
            self.requests.spawn(async move {
                let frame = run_tool(&ctx, id.clone(), &tool, &payload).await;
                (id, control, frame)
            });
            return true;
        }
        let frame = run_inline(&self.ctx, &mut self.terminals, id, &tool, &payload).await;
        self.send(&frame).await
    }

    /// Releases everything the socket held once it has ended.
    pub(crate) async fn close(&mut self) {
        self.ctx.dashboard_cancel.cancel();
        // Dropping a transport wait does not establish remote cancellation.
        self.requests.shutdown().await;
        self.terminals.close_all(&self.ctx).await;
        debug!(tenant = %self.ctx.session.tenant, "websocket closed");
    }
}
