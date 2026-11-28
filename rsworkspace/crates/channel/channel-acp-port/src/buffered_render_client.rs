#[cfg(test)]
mod tests;

use acp_nats::ClientHandler;
use agent_client_protocol::v1::schema::{
    ContentBlock, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionNotification,
    SessionUpdate,
};
use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{debug, warn};

/// A chat bridge's ACP client half: receives agent session notifications and
/// accumulates streamed text per session; the bridge's message loop flushes
/// the buffer to its platform when the prompt turn ends. `ClientHandler` requires `Sync`, so
/// the per-session buffers use a `Mutex` (no lock is held across an await).
pub struct BufferedRenderClient {
    buffers: Mutex<HashMap<String, String>>,
}

impl BufferedRenderClient {
    pub fn new() -> Self {
        Self {
            buffers: Mutex::new(HashMap::new()),
        }
    }

    /// Take the accumulated agent text for a session, if any.
    pub fn take_buffer(&self, session_id: &str) -> Option<String> {
        self.buffers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id)
            .filter(|s| !s.trim().is_empty())
    }

    /// Drop whatever a session accumulated without sending it. A released
    /// session can still have text in flight, and none of it belongs to the
    /// conversation that moved on.
    pub fn discard(&self, session_id: &str) {
        self.buffers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id);
    }
}

impl Default for BufferedRenderClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ClientHandler for BufferedRenderClient {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        // A chat channel has no interactive permission surface yet; refuse
        // rather than silently grant.
        warn!(session_id = %args.session_id, "Agent requested permission; cancelling (no permission UI on this channel)");
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn session_notification(&self, args: SessionNotification) -> agent_client_protocol::Result<()> {
        match args.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(text) = chunk.content {
                    let mut buffers = self.buffers.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    match buffers.get_mut(&*args.session_id.0) {
                        Some(buffer) => buffer.push_str(&text.text),
                        None => {
                            buffers.insert(args.session_id.to_string(), text.text);
                        }
                    }
                }
            }
            other => {
                debug!(session_id = %args.session_id, update = ?std::mem::discriminant(&other), "Ignoring non-message session update");
            }
        }
        Ok(())
    }
}
