//! Conversation state management
//!
//! Manages conversation history and context for each session.
//! Supports both in-memory storage (default) and persistent NATS KV storage.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::llm::Message;

/// Maximum messages to keep in history per session
const MAX_HISTORY_SIZE: usize = 20;

/// Conversation manager
///
/// Use `ConversationManager::new()` for in-memory storage (lost on restart).
/// Use `ConversationManager::with_kv()` for persistent storage via NATS KV.
pub struct ConversationManager {
    /// In-memory fallback / cache
    sessions: Arc<RwLock<HashMap<String, Vec<Message>>>>,
    /// Optional NATS KV for persistence
    kv: Option<async_nats::jetstream::kv::Store>,
}

impl ConversationManager {
    /// Create a new in-memory conversation manager
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kv: None,
        }
    }

    /// Create a conversation manager backed by NATS KV for persistence
    pub fn with_kv(kv: async_nats::jetstream::kv::Store) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            kv: Some(kv),
        }
    }

    /// Add a message to conversation history
    pub async fn add_message(&self, session_id: &str, role: &str, content: &str) {
        let mut history = self.get_history(session_id).await;

        history.push(Message {
            role: role.to_string(),
            content: content.to_string(),
        });

        // Trim history if too long
        if history.len() > MAX_HISTORY_SIZE {
            let start = history.len() - MAX_HISTORY_SIZE;
            history = history[start..].to_vec();
        }

        if let Some(ref kv) = self.kv {
            match serde_json::to_vec(&history) {
                Ok(bytes) => {
                    if let Err(e) = kv.put(session_id, bytes.into()).await {
                        tracing::warn!("Failed to persist conversation for {}: {}", session_id, e);
                        // Fall through to in-memory update
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to serialize conversation for {}: {}", session_id, e);
                }
            }
        }

        // Always keep in-memory copy for fast access
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.to_string(), history);
    }

    /// Get conversation history for a session
    pub async fn get_history(&self, session_id: &str) -> Vec<Message> {
        // Try in-memory cache first
        {
            let sessions = self.sessions.read().await;
            if let Some(history) = sessions.get(session_id) {
                return history.clone();
            }
        }

        // If KV is available, try loading from persistent storage
        if let Some(ref kv) = self.kv {
            match kv.get(session_id).await {
                Ok(Some(bytes)) => {
                    match serde_json::from_slice::<Vec<Message>>(&bytes) {
                        Ok(history) => {
                            // Populate cache
                            let mut sessions = self.sessions.write().await;
                            sessions.insert(session_id.to_string(), history.clone());
                            return history;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to deserialize conversation for {}: {}", session_id, e);
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("Failed to load conversation for {}: {}", session_id, e);
                }
            }
        }

        vec![]
    }

    /// Clear conversation history for a session
    pub async fn clear_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);

        if let Some(ref kv) = self.kv {
            if let Err(e) = kv.delete(session_id).await {
                tracing::warn!("Failed to delete conversation from KV for {}: {}", session_id, e);
            }
        }
    }

    /// Get number of active sessions (in-memory count)
    pub async fn active_sessions(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }
}

impl Default for ConversationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_new_has_no_sessions() {
        let mgr = ConversationManager::new();
        assert_eq!(mgr.active_sessions().await, 0);
    }

    #[tokio::test]
    async fn test_get_history_empty_for_unknown_session() {
        let mgr = ConversationManager::new();
        let history = mgr.get_history("nonexistent").await;
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_add_message_creates_session() {
        let mgr = ConversationManager::new();
        mgr.add_message("session1", "user", "Hello").await;
        assert_eq!(mgr.active_sessions().await, 1);
    }

    #[tokio::test]
    async fn test_add_message_stores_role_and_content() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "user", "Hi there").await;
        mgr.add_message("s1", "assistant", "Hello!").await;

        let history = mgr.get_history("s1").await;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "Hi there");
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[1].content, "Hello!");
    }

    #[tokio::test]
    async fn test_sessions_are_isolated() {
        let mgr = ConversationManager::new();
        mgr.add_message("session_a", "user", "Message A").await;
        mgr.add_message("session_b", "user", "Message B").await;

        assert_eq!(mgr.active_sessions().await, 2);
        let hist_a = mgr.get_history("session_a").await;
        let hist_b = mgr.get_history("session_b").await;
        assert_eq!(hist_a.len(), 1);
        assert_eq!(hist_a[0].content, "Message A");
        assert_eq!(hist_b.len(), 1);
        assert_eq!(hist_b[0].content, "Message B");
    }

    #[tokio::test]
    async fn test_history_trimmed_at_max_size() {
        let mgr = ConversationManager::new();

        // Add MAX_HISTORY_SIZE + 5 messages
        for i in 0..(MAX_HISTORY_SIZE + 5) {
            mgr.add_message("s", "user", &format!("msg {}", i)).await;
        }

        let history = mgr.get_history("s").await;
        assert_eq!(history.len(), MAX_HISTORY_SIZE);
        // Should keep the LAST MAX_HISTORY_SIZE messages
        assert_eq!(history[0].content, "msg 5");
        assert_eq!(history[MAX_HISTORY_SIZE - 1].content, format!("msg {}", MAX_HISTORY_SIZE + 4));
    }

    #[tokio::test]
    async fn test_history_not_trimmed_below_max() {
        let mgr = ConversationManager::new();
        for i in 0..MAX_HISTORY_SIZE {
            mgr.add_message("s", "user", &format!("msg {}", i)).await;
        }
        let history = mgr.get_history("s").await;
        assert_eq!(history.len(), MAX_HISTORY_SIZE);
        assert_eq!(history[0].content, "msg 0");
    }

    #[tokio::test]
    async fn test_clear_session_removes_history() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "user", "Hello").await;
        mgr.add_message("s2", "user", "World").await;

        mgr.clear_session("s1").await;

        assert_eq!(mgr.active_sessions().await, 1);
        assert!(mgr.get_history("s1").await.is_empty());
        assert_eq!(mgr.get_history("s2").await.len(), 1);
    }

    #[tokio::test]
    async fn test_clear_nonexistent_session_is_noop() {
        let mgr = ConversationManager::new();
        mgr.clear_session("ghost").await; // should not panic
        assert_eq!(mgr.active_sessions().await, 0);
    }

    #[tokio::test]
    async fn test_default_is_same_as_new() {
        let mgr = ConversationManager::default();
        assert_eq!(mgr.active_sessions().await, 0);
    }
}
