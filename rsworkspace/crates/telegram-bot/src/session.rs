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
    pub async fn get_or_create(
        &self,
        session_id: &SessionId,
        chat_id: i64,
        user_id: Option<i64>,
    ) -> Result<SessionState> {
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

    // ── NATS / JetStream KV integration ──────────────────────────────────────

    mod nats_integration {
        use super::super::*;
        use async_nats::jetstream;
        use telegram_types::SessionId;

        const NATS_URL: &str = "nats://localhost:14222";

        async fn make_manager() -> Option<SessionManager> {
            let client = async_nats::connect(NATS_URL).await.ok()?;
            let js = jetstream::new(client);
            let bucket = format!("test-sess-{}", uuid::Uuid::new_v4().simple());
            let kv = js
                .create_key_value(jetstream::kv::Config {
                    bucket,
                    ..Default::default()
                })
                .await
                .ok()?;
            Some(SessionManager::new(kv))
        }

        #[tokio::test]
        async fn test_get_or_create_new_session() {
            let Some(mgr) = make_manager().await else {
                eprintln!("SKIP: NATS not available");
                return;
            };

            let sid = SessionId::from("tg-private-1".to_string());
            let state = mgr.get_or_create(&sid, 1, Some(42)).await.unwrap();

            assert_eq!(state.session_id, "tg-private-1");
            assert_eq!(state.chat_id, 1);
            assert_eq!(state.user_id, Some(42));
            assert_eq!(state.message_count, 1);
        }

        #[tokio::test]
        async fn test_get_or_create_existing_increments_count() {
            let Some(mgr) = make_manager().await else {
                eprintln!("SKIP: NATS not available");
                return;
            };

            let sid = SessionId::from("tg-private-2".to_string());

            let first = mgr.get_or_create(&sid, 2, Some(10)).await.unwrap();
            assert_eq!(first.message_count, 1);

            let second = mgr.get_or_create(&sid, 2, Some(10)).await.unwrap();
            assert_eq!(second.message_count, 2);
            assert_eq!(second.session_id, "tg-private-2");
            assert_eq!(second.chat_id, 2);
            assert!(second.last_activity >= first.last_activity);
        }

        #[tokio::test]
        async fn test_get_returns_stored_session() {
            let Some(mgr) = make_manager().await else {
                eprintln!("SKIP: NATS not available");
                return;
            };

            let sid = SessionId::from("tg-private-3".to_string());

            // Nothing stored yet
            let none = mgr.get(&sid).await.unwrap();
            assert!(none.is_none());

            // Create it
            mgr.get_or_create(&sid, 3, None).await.unwrap();

            // Now get() should return it
            let stored = mgr.get(&sid).await.unwrap().expect("session must exist");
            assert_eq!(stored.session_id, "tg-private-3");
            assert_eq!(stored.chat_id, 3);
            assert_eq!(stored.message_count, 1);
        }

        #[tokio::test]
        async fn test_update_overwrites_session() {
            let Some(mgr) = make_manager().await else {
                eprintln!("SKIP: NATS not available");
                return;
            };

            let sid = SessionId::from("tg-private-4".to_string());
            let mut state = mgr.get_or_create(&sid, 4, Some(99)).await.unwrap();

            state.message_count = 100;
            state.user_id = None;
            mgr.update(&state).await.unwrap();

            let loaded = mgr.get(&sid).await.unwrap().expect("session must exist");
            assert_eq!(loaded.message_count, 100);
            assert!(loaded.user_id.is_none());
        }

        #[tokio::test]
        async fn test_delete_removes_session() {
            let Some(mgr) = make_manager().await else {
                eprintln!("SKIP: NATS not available");
                return;
            };

            let sid = SessionId::from("tg-private-5".to_string());
            mgr.get_or_create(&sid, 5, Some(7)).await.unwrap();

            // Verify it exists
            assert!(mgr.get(&sid).await.unwrap().is_some());

            // Delete
            mgr.delete(&sid).await.unwrap();

            // Verify it's gone
            let after = mgr.get(&sid).await.unwrap();
            assert!(after.is_none());
        }

        #[tokio::test]
        async fn test_get_or_create_preserves_created_at() {
            let Some(mgr) = make_manager().await else {
                eprintln!("SKIP: NATS not available");
                return;
            };

            let sid = SessionId::from("tg-private-6".to_string());

            let first = mgr.get_or_create(&sid, 6, Some(1)).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let second = mgr.get_or_create(&sid, 6, Some(1)).await.unwrap();

            // created_at must not change
            assert_eq!(second.created_at, first.created_at);
            // but message_count and last_activity must update
            assert_eq!(second.message_count, 2);
            assert!(second.last_activity >= first.last_activity);
        }

        #[tokio::test]
        async fn test_session_persists_across_manager_instances() {
            let client = match async_nats::connect(NATS_URL).await {
                Ok(c) => c,
                Err(_) => {
                    eprintln!("SKIP: NATS not available");
                    return;
                }
            };
            let js = jetstream::new(client.clone());
            let bucket = format!("test-sess-{}", uuid::Uuid::new_v4().simple());

            // First manager: create session
            let kv1 = js
                .create_key_value(jetstream::kv::Config {
                    bucket: bucket.clone(),
                    ..Default::default()
                })
                .await
                .unwrap();
            let mgr1 = SessionManager::new(kv1);
            let sid = SessionId::from("tg-private-7".to_string());
            let created = mgr1.get_or_create(&sid, 7, Some(55)).await.unwrap();

            // Second manager: open same bucket, read session
            let kv2 = js.get_key_value(&bucket).await.unwrap();
            let mgr2 = SessionManager::new(kv2);
            let loaded = mgr2.get(&sid).await.unwrap().expect("session must persist");

            assert_eq!(loaded.session_id, created.session_id);
            assert_eq!(loaded.chat_id, created.chat_id);
            assert_eq!(loaded.user_id, created.user_id);
            assert_eq!(loaded.message_count, created.message_count);
            assert_eq!(loaded.created_at, created.created_at);
        }

        #[tokio::test]
        async fn test_get_or_create_anonymous_session() {
            let Some(mgr) = make_manager().await else {
                eprintln!("SKIP: NATS not available");
                return;
            };

            // Channels and anonymous group messages have no user_id
            let sid = SessionId::from("tg-channel-999".to_string());
            let state = mgr.get_or_create(&sid, -100999, None).await.unwrap();

            assert_eq!(state.chat_id, -100999);
            assert!(state.user_id.is_none());
            assert_eq!(state.message_count, 1);

            // Second call: still no user_id
            let state2 = mgr.get_or_create(&sid, -100999, None).await.unwrap();
            assert!(state2.user_id.is_none());
            assert_eq!(state2.message_count, 2);
        }
    }
}
