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
