//! Session management for Telegram bot

use anyhow::Result;
use async_nats::jetstream::kv::Store;
use serde::{Deserialize, Serialize};
use telegram_types::SessionId;
use tracing::{debug, warn};

/// Session manager
#[derive(Clone)]
pub struct SessionManager {
    kv: Store,
}

/// Session state stored in JetStream KV
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub created_at: i64,
    pub last_activity: i64,
    pub message_count: u64,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(kv: Store) -> Self {
        Self { kv }
    }

    /// Get or create a session
    pub async fn get_or_create(&self, session_id: &SessionId, chat_id: i64, user_id: Option<i64>) -> Result<SessionState> {
        let key = session_id.as_str();

        // Try to get existing session
        match self.kv.get(key).await {
            Ok(Some(entry)) => {
                debug!("Found existing session: {}", session_id);
                let mut state: SessionState = serde_json::from_slice(&entry)?;

                // Update last activity
                state.last_activity = chrono::Utc::now().timestamp();
                state.message_count += 1;

                // Save updated state
                self.kv.put(key, serde_json::to_vec(&state)?.into()).await?;

                Ok(state)
            }
            _ => {
                debug!("Creating new session: {}", session_id);
                let now = chrono::Utc::now().timestamp();
                let state = SessionState {
                    session_id: session_id.to_string(),
                    chat_id,
                    user_id,
                    created_at: now,
                    last_activity: now,
                    message_count: 1,
                };

                // Save new session
                self.kv.put(key, serde_json::to_vec(&state)?.into()).await?;

                Ok(state)
            }
        }
    }

    /// Get a session state
    pub async fn get(&self, session_id: &SessionId) -> Result<Option<SessionState>> {
        let key = session_id.as_str();

        match self.kv.get(key).await {
            Ok(Some(entry)) => {
                let state: SessionState = serde_json::from_slice(&entry)?;
                Ok(Some(state))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                warn!("Failed to get session {}: {}", session_id, e);
                Ok(None)
            }
        }
    }

    /// Update session state
    pub async fn update(&self, state: &SessionState) -> Result<()> {
        let key = state.session_id.as_str();
        self.kv.put(key, serde_json::to_vec(state)?.into()).await?;
        Ok(())
    }

    /// Delete a session
    pub async fn delete(&self, session_id: &SessionId) -> Result<()> {
        let key = session_id.as_str();
        self.kv.delete(key).await?;
        debug!("Deleted session: {}", session_id);
        Ok(())
    }
}
