use acp_nats::ClientHandler;
use agent_client_protocol::schema::v1::{
    ContentBlock, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionNotification,
    SessionUpdate,
};
use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{debug, warn};

/// The Telegram limit for a single message.
pub const TEXT_CHUNK_LIMIT: usize = 4096;

/// The bridge's ACP client half: receives agent session notifications and
/// accumulates streamed text per session; the message loop flushes the buffer
/// to Telegram when the prompt turn ends. `ClientHandler` requires `Sync`, so
/// the per-session buffers use a `Mutex` (no lock is held across an await).
pub struct TelegramRenderClient {
    buffers: Mutex<HashMap<String, String>>,
}

impl TelegramRenderClient {
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

impl Default for TelegramRenderClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ClientHandler for TelegramRenderClient {
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
        let session_id = args.session_id.to_string();
        match args.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(text) = chunk.content {
                    self.buffers
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .entry(session_id)
                        .or_default()
                        .push_str(&text.text);
                }
            }
            other => {
                debug!(session_id = %session_id, update = ?std::mem::discriminant(&other), "Ignoring non-message session update");
            }
        }
        Ok(())
    }
}

/// Split text at Telegram's message size limit on char boundaries.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count == limit {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        current.push(ch);
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_splits_on_char_boundaries() {
        let text = "ab".repeat(3000);
        let chunks = chunk_text(&text, TEXT_CHUNK_LIMIT);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chars().count(), TEXT_CHUNK_LIMIT);
        assert_eq!(chunks[1].chars().count(), 6000 - TEXT_CHUNK_LIMIT);
    }

    #[test]
    fn chunk_text_handles_multibyte() {
        let text = "\u{1F980}".repeat(10);
        let chunks = chunk_text(&text, 4);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 4));
    }
}
