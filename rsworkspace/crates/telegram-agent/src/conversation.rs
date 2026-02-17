//! Conversation state management
//!
//! Manages conversation history and context for each session

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::llm::Message;

/// Maximum messages to keep in history per session
const MAX_HISTORY_SIZE: usize = 20;

/// Conversation manager
pub struct ConversationManager {
    sessions: Arc<RwLock<HashMap<String, Vec<Message>>>>,
}

impl ConversationManager {
    /// Create a new conversation manager
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a message to conversation history
    pub async fn add_message(&self, session_id: &str, role: &str, content: &str) {
        let mut sessions = self.sessions.write().await;
        let history = sessions.entry(session_id.to_string()).or_insert_with(Vec::new);

        history.push(Message {
            role: role.to_string(),
            content: content.to_string(),
        });

        // Trim history if too long
        if history.len() > MAX_HISTORY_SIZE {
            let start = history.len() - MAX_HISTORY_SIZE;
            *history = history[start..].to_vec();
        }
    }

    /// Get conversation history for a session
    pub async fn get_history(&self, session_id: &str) -> Vec<Message> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned().unwrap_or_default()
    }

    /// Clear conversation history for a session
    pub async fn clear_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
    }

    /// Get number of active sessions
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
