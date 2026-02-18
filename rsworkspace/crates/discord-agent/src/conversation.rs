//! Conversation state management
//!
//! Manages conversation history and context for each session.
//! Supports both in-memory storage (default) and persistent NATS KV storage.

use crate::llm::Message;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant};

/// Maximum messages to keep in history per session
const MAX_HISTORY_SIZE: usize = 20;

/// How often the in-memory cleanup task runs.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(3600); // 1 hour

/// Conversation manager
///
/// Use `ConversationManager::new()` for in-memory storage (lost on restart).
/// Use `ConversationManager::with_kv()` for persistent storage via NATS KV.
pub struct ConversationManager {
    /// In-memory fallback / cache
    sessions: Arc<RwLock<HashMap<String, Vec<Message>>>>,
    /// Tracks the last time each session was accessed (for TTL eviction)
    last_access: Arc<RwLock<HashMap<String, Instant>>>,
    /// Optional NATS KV for persistence
    kv: Option<async_nats::jetstream::kv::Store>,
}

impl ConversationManager {
    /// Create a new in-memory conversation manager
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            last_access: Arc::new(RwLock::new(HashMap::new())),
            kv: None,
        }
    }

    /// Create a conversation manager backed by NATS KV for persistence
    pub fn with_kv(kv: async_nats::jetstream::kv::Store) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            last_access: Arc::new(RwLock::new(HashMap::new())),
            kv: Some(kv),
        }
    }

    /// Start a background task that evicts in-memory sessions older than `max_age`.
    ///
    /// The NATS KV TTL is set separately at bucket-creation time in `main.rs`.
    /// This handles the in-memory cache so long-running processes don't leak RAM.
    pub fn start_cleanup(&self, max_age: Duration) {
        let sessions = self.sessions.clone();
        let last_access = self.last_access.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CLEANUP_INTERVAL).await;

                let expired: Vec<String> = {
                    let la = last_access.read().await;
                    la.iter()
                        .filter(|(_, &ts)| ts.elapsed() > max_age)
                        .map(|(k, _)| k.clone())
                        .collect()
                };

                if !expired.is_empty() {
                    let mut s = sessions.write().await;
                    let mut la = last_access.write().await;
                    for key in &expired {
                        s.remove(key);
                        la.remove(key);
                    }
                    tracing::info!(
                        "Evicted {} stale in-memory conversation session(s)",
                        expired.len()
                    );
                }
            }
        });
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
        drop(sessions);

        self.touch(session_id).await;
    }

    /// Get conversation history for a session
    pub async fn get_history(&self, session_id: &str) -> Vec<Message> {
        // Try in-memory cache first (clone while holding lock, then release)
        let cached = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        if let Some(history) = cached {
            self.touch(session_id).await;
            return history;
        }

        // If KV is available, try loading from persistent storage
        if let Some(ref kv) = self.kv {
            match kv.get(session_id).await {
                Ok(Some(bytes)) => match serde_json::from_slice::<Vec<Message>>(&bytes) {
                    Ok(history) => {
                        let mut sessions = self.sessions.write().await;
                        sessions.insert(session_id.to_string(), history.clone());
                        drop(sessions);
                        self.touch(session_id).await;
                        return history;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to deserialize conversation for {}: {}",
                            session_id,
                            e
                        );
                    }
                },
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("Failed to load conversation for {}: {}", session_id, e);
                }
            }
        }

        vec![]
    }

    /// Overwrite conversation history for a session (used for edits/deletions)
    pub async fn save_history(&self, session_id: &str, history: Vec<Message>) {
        if let Some(ref kv) = self.kv {
            match serde_json::to_vec(&history) {
                Ok(bytes) => {
                    if let Err(e) = kv.put(session_id, bytes.into()).await {
                        tracing::warn!("Failed to persist conversation for {}: {}", session_id, e);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to serialize conversation for {}: {}", session_id, e);
                }
            }
        }

        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.to_string(), history);
        drop(sessions);

        self.touch(session_id).await;
    }

    /// Clear conversation history for a session
    pub async fn clear_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        drop(sessions);

        let mut la = self.last_access.write().await;
        la.remove(session_id);
        drop(la);

        if let Some(ref kv) = self.kv {
            if let Err(e) = kv.delete(session_id).await {
                tracing::warn!(
                    "Failed to delete conversation from KV for {}: {}",
                    session_id,
                    e
                );
            }
        }
    }

    /// Get number of active sessions (in-memory count)
    pub async fn active_sessions(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    // ── Private helpers ────────────────────────────────────────────────────────

    async fn touch(&self, session_id: &str) {
        let mut la = self.last_access.write().await;
        la.insert(session_id.to_string(), Instant::now());
    }
}

impl Default for ConversationManager {
    fn default() -> Self {
        Self::new()
    }
}
