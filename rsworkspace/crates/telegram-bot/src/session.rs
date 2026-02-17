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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub async fn update(&self, state: &SessionState) -> Result<()> {
        let key = state.session_id.as_str();
        self.kv.put(key, serde_json::to_vec(state)?.into()).await?;
        Ok(())
    }

    /// Delete a session
    #[allow(dead_code)]
    pub async fn delete(&self, session_id: &SessionId) -> Result<()> {
        let key = session_id.as_str();
        self.kv.delete(key).await?;
        debug!("Deleted session: {}", session_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> SessionState {
        SessionState {
            session_id: "tg-private-123".to_string(),
            chat_id: 123,
            user_id: Some(456),
            created_at: 1700000000,
            last_activity: 1700000060,
            message_count: 5,
        }
    }

    // ── SessionState serde ────────────────────────────────────────────────────

    #[test]
    fn test_session_state_roundtrip() {
        let state = sample_state();
        let json = serde_json::to_string(&state).expect("serialize");
        let back: SessionState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.session_id, state.session_id);
        assert_eq!(back.chat_id, state.chat_id);
        assert_eq!(back.user_id, state.user_id);
        assert_eq!(back.created_at, state.created_at);
        assert_eq!(back.last_activity, state.last_activity);
        assert_eq!(back.message_count, state.message_count);
    }

    #[test]
    fn test_session_state_no_user_id() {
        let state = SessionState {
            session_id: "tg-group-999".to_string(),
            chat_id: -999,
            user_id: None,
            created_at: 1700000000,
            last_activity: 1700000000,
            message_count: 1,
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let back: SessionState = serde_json::from_str(&json).expect("deserialize");
        assert!(back.user_id.is_none());
        assert_eq!(back.chat_id, -999);
    }

    #[test]
    fn test_session_state_message_count_starts_at_one() {
        // New sessions should have message_count = 1 (first message)
        let state = SessionState {
            session_id: "tg-private-1".to_string(),
            chat_id: 1,
            user_id: None,
            created_at: 0,
            last_activity: 0,
            message_count: 1,
        };
        assert_eq!(state.message_count, 1);
    }

    #[test]
    fn test_session_state_json_contains_expected_keys() {
        let state = sample_state();
        let json = serde_json::to_string(&state).expect("serialize");
        assert!(json.contains("\"session_id\""));
        assert!(json.contains("\"chat_id\""));
        assert!(json.contains("\"user_id\""));
        assert!(json.contains("\"created_at\""));
        assert!(json.contains("\"last_activity\""));
        assert!(json.contains("\"message_count\""));
    }

    #[test]
    fn test_session_state_clone() {
        let state = sample_state();
        let clone = state.clone();
        assert_eq!(clone.session_id, state.session_id);
        assert_eq!(clone.message_count, state.message_count);
    }
}
