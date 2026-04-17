//! Interactive chat session storage — backed by NATS JetStream Key-Value.
//!
//! Each session stores the full Anthropic conversation history so that every
//! new user message continues from where the previous turn left off.
//!
//! # Bucket
//! All sessions are stored in the `SESSIONS` KV bucket.
//! Key format: `{tenant_id}.{session_id}`.

use std::future::Future;
use std::pin::Pin;

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::agent_loop::Message;

/// NATS KV bucket name for chat sessions.
pub const SESSIONS_BUCKET: &str = "SESSIONS";

/// An interactive agent chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    /// Unique session ID (UUID v4).
    pub id: String,
    /// Tenant that owns this session.
    pub tenant_id: String,
    /// Display name — auto-generated from the first user message if not set.
    pub name: String,
    /// Anthropic model override (None → global default).
    #[serde(default)]
    pub model: Option<String>,
    /// Built-in tool names available (empty = all).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Memory file path override (None → global default).
    #[serde(default)]
    pub memory_path: Option<String>,
    /// Full Anthropic conversation history — every user turn, tool exchange,
    /// and assistant response is persisted here.
    #[serde(default)]
    pub messages: Vec<Message>,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 last-updated timestamp.
    pub updated_at: String,
    /// Unix epoch seconds when the session was created (used to compute duration_ms).
    #[serde(default)]
    pub started_at_secs: u64,
    /// Cumulative duration in milliseconds from creation to last message turn.
    #[serde(default)]
    pub duration_ms: u64,
    /// ID of the agent definition from the console (populated when `AGENT_ID` is set).
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// An error from the session store.
#[derive(Debug)]
pub struct SessionStoreError(pub String);

impl std::fmt::Display for SessionStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SessionStoreError {}

/// NATS KV-backed store for [`ChatSession`]s.
#[derive(Clone)]
pub struct SessionStore {
    kv: kv::Store,
}

fn kv_key(tenant_id: &str, id: &str) -> String {
    format!("{tenant_id}.{id}")
}

impl SessionStore {
    /// Open (or create) the `SESSIONS` KV bucket.
    pub async fn open(js: &jetstream::Context) -> Result<Self, SessionStoreError> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: SESSIONS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| SessionStoreError(e.to_string()))?;
        Ok(Self { kv })
    }

    /// Persist a session (create or overwrite).
    pub async fn put(&self, session: &ChatSession) -> Result<(), SessionStoreError> {
        let key = kv_key(&session.tenant_id, &session.id);
        let bytes = serde_json::to_vec(session).map_err(|e| SessionStoreError(e.to_string()))?;
        self.kv
            .put(&key, Bytes::from(bytes))
            .await
            .map_err(|e| SessionStoreError(e.to_string()))?;
        Ok(())
    }

    /// Fetch a single session by tenant + id.
    pub async fn get(
        &self,
        tenant_id: &str,
        id: &str,
    ) -> Result<Option<ChatSession>, SessionStoreError> {
        let key = kv_key(tenant_id, id);
        match self
            .kv
            .get(&key)
            .await
            .map_err(|e| SessionStoreError(e.to_string()))?
        {
            None => Ok(None),
            Some(bytes) => {
                let s = serde_json::from_slice::<ChatSession>(&bytes)
                    .map_err(|e| SessionStoreError(e.to_string()))?;
                Ok(Some(s))
            }
        }
    }

    /// Delete a session.
    pub async fn delete(&self, tenant_id: &str, id: &str) -> Result<(), SessionStoreError> {
        let key = kv_key(tenant_id, id);
        self.kv
            .delete(&key)
            .await
            .map_err(|e| SessionStoreError(e.to_string()))?;
        Ok(())
    }

    /// Return all sessions for `tenant_id`, sorted newest-first by `updated_at`.
    pub async fn list(&self, tenant_id: &str) -> Result<Vec<ChatSession>, SessionStoreError> {
        let prefix = format!("{tenant_id}.");
        let mut keys = self
            .kv
            .keys()
            .await
            .map_err(|e| SessionStoreError(e.to_string()))?;
        let mut result = Vec::new();

        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| SessionStoreError(e.to_string()))?;
            if !key.starts_with(&prefix) {
                continue;
            }
            let id = &key[prefix.len()..];
            match self.get(tenant_id, id).await {
                Ok(Some(s)) => result.push(s),
                Ok(None) => {}
                Err(e) => warn!(key, error = %e, "Skipping unreadable session"),
            }
        }

        result.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(result)
    }
}

// ── Repository trait ──────────────────────────────────────────────────────────

/// Abstraction over a session store.
///
/// Implementing this trait allows replacing the real NATS KV backend with a
/// lightweight in-memory fake in unit tests.
pub trait SessionRepository: Clone + Send + Sync + 'static {
    fn put<'a>(
        &'a self,
        session: &'a ChatSession,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>>;

    fn get<'a>(
        &'a self,
        tenant_id: &'a str,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ChatSession>, SessionStoreError>> + Send + 'a>>;

    fn delete<'a>(
        &'a self,
        tenant_id: &'a str,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>>;

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ChatSession>, SessionStoreError>> + Send + 'a>>;
}

impl SessionRepository for SessionStore {
    fn put<'a>(
        &'a self,
        session: &'a ChatSession,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move { self.put(session).await })
    }

    fn get<'a>(
        &'a self,
        tenant_id: &'a str,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ChatSession>, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move { self.get(tenant_id, id).await })
    }

    fn delete<'a>(
        &'a self,
        tenant_id: &'a str,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move { self.delete(tenant_id, id).await })
    }

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ChatSession>, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move { self.list(tenant_id).await })
    }
}

// ── In-memory mock (test-only) ────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// HashMap-backed in-memory session store for unit tests.
    #[derive(Clone, Default)]
    pub struct MockSessionStore {
        data: Arc<Mutex<HashMap<String, ChatSession>>>,
    }

    impl MockSessionStore {
        pub fn new() -> Self {
            Self::default()
        }

        /// Pre-populate the store with a session.
        pub fn insert(&self, session: ChatSession) {
            let key = format!("{}.{}", session.tenant_id, session.id);
            self.data.lock().unwrap().insert(key, session);
        }

        /// Snapshot the current store contents.
        pub fn snapshot(&self) -> HashMap<String, ChatSession> {
            self.data.lock().unwrap().clone()
        }
    }

    /// Session store that always returns an error — used to test 500 paths.
    #[derive(Clone, Default)]
    pub struct ErrorSessionStore;

    impl ErrorSessionStore {
        pub fn new() -> Self {
            Self
        }
    }

    impl SessionRepository for ErrorSessionStore {
        fn put<'a>(
            &'a self,
            _session: &'a ChatSession,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            Box::pin(async move { Err(SessionStoreError("injected put error".into())) })
        }

        fn get<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<ChatSession>, SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async move { Err(SessionStoreError("injected get error".into())) })
        }

        fn delete<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            Box::pin(async move { Err(SessionStoreError("injected delete error".into())) })
        }

        fn list<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<ChatSession>, SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async move { Err(SessionStoreError("injected list error".into())) })
        }
    }

    /// Session store that succeeds on `get()` (returning a dummy session) but
    /// fails on `put()` and `delete()`. Used to test the second-store-call 500
    /// paths in `update_session` and `delete_session`.
    #[derive(Clone, Default)]
    pub struct GetOkPutErrorSessionStore;

    impl GetOkPutErrorSessionStore {
        pub fn new() -> Self {
            Self
        }

        fn dummy_session() -> ChatSession {
            ChatSession {
                id: "dummy".into(),
                tenant_id: "acme".into(),
                name: "Dummy".into(),
                model: None,
                tools: vec![],
                memory_path: None,
                messages: vec![],
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                started_at_secs: 0,
                duration_ms: 0,
                agent_id: None,
            }
        }
    }

    impl SessionRepository for GetOkPutErrorSessionStore {
        fn put<'a>(
            &'a self,
            _session: &'a ChatSession,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            Box::pin(async move { Err(SessionStoreError("injected put error".into())) })
        }

        fn get<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<ChatSession>, SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async move { Ok(Some(Self::dummy_session())) })
        }

        fn delete<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            Box::pin(async move { Err(SessionStoreError("injected delete error".into())) })
        }

        fn list<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<ChatSession>, SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async move { Err(SessionStoreError("injected list error".into())) })
        }
    }

    impl SessionRepository for MockSessionStore {
        fn put<'a>(
            &'a self,
            session: &'a ChatSession,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let session = session.clone();
            Box::pin(async move {
                let key = format!("{}.{}", session.tenant_id, session.id);
                data.lock().unwrap().insert(key, session);
                Ok(())
            })
        }

        fn get<'a>(
            &'a self,
            tenant_id: &'a str,
            id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<ChatSession>, SessionStoreError>> + Send + 'a>>
        {
            let data = Arc::clone(&self.data);
            let key = format!("{tenant_id}.{id}");
            Box::pin(async move { Ok(data.lock().unwrap().get(&key).cloned()) })
        }

        fn delete<'a>(
            &'a self,
            tenant_id: &'a str,
            id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let key = format!("{tenant_id}.{id}");
            Box::pin(async move {
                data.lock().unwrap().remove(&key);
                Ok(())
            })
        }

        fn list<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<ChatSession>, SessionStoreError>> + Send + 'a>>
        {
            let data = Arc::clone(&self.data);
            let prefix = format!("{tenant_id}.");
            Box::pin(async move {
                let guard = data.lock().unwrap();
                let mut sessions: Vec<ChatSession> = guard
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, v)| v.clone())
                    .collect();
                sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                Ok(sessions)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::ContentBlock;

    fn sample_session(id: &str) -> ChatSession {
        ChatSession {
            id: id.to_string(),
            tenant_id: "acme".to_string(),
            name: "Test session".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            messages: vec![
                Message::user_text("Hello"),
                Message::assistant(vec![ContentBlock::Text {
                    text: "Hi there!".to_string(),
                }]),
            ],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            started_at_secs: 0,
            duration_ms: 0,
            agent_id: None,
        }
    }

    #[test]
    fn session_round_trips_json() {
        let s = sample_session("sess-1");
        let json = serde_json::to_string(&s).unwrap();
        let s2: ChatSession = serde_json::from_str(&json).unwrap();
        assert_eq!(s2.id, "sess-1");
        assert_eq!(s2.messages.len(), 2);
        assert_eq!(s2.messages[0].role, "user");
        assert_eq!(s2.messages[1].role, "assistant");
    }

    #[test]
    fn session_messages_default_to_empty() {
        let json = r#"{"id":"x","tenant_id":"t","name":"n",
                       "created_at":"2026-01-01T00:00:00Z",
                       "updated_at":"2026-01-01T00:00:00Z"}"#;
        let s: ChatSession = serde_json::from_str(json).unwrap();
        assert!(s.messages.is_empty());
    }

    #[test]
    fn kv_key_format() {
        assert_eq!(super::kv_key("acme", "sess-1"), "acme.sess-1");
    }

    #[test]
    fn session_store_error_display() {
        let e = SessionStoreError("bucket gone".to_string());
        assert!(e.to_string().contains("bucket gone"));
    }
}
