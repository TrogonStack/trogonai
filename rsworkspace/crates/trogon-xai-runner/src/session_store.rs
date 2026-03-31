use async_trait::async_trait;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::client::Message;

/// Serializable session state persisted in the session store.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct XaiSessionData {
    pub cwd: String,
    pub model: Option<String>,
    pub history: Vec<Message>,
    /// xAI API key for this session.
    pub api_key: Option<String>,
    /// Optional system prompt injected as the first message of every turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Server-side tools enabled for this session (Responses API `tools` array).
    /// E.g. `["web_search", "x_search", "code_interpreter"]`.
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    /// ID of the last response from xAI, used as `previous_response_id` on the
    /// next turn to avoid re-sending full history. Cleared when the session is
    /// forked (fork starts fresh without prior context reference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_response_id: Option<String>,
}

/// Persistent store for session data.
#[async_trait(?Send)]
pub trait SessionStore {
    /// Returns `None` if the session does not exist.
    async fn get(&self, id: &str) -> Option<XaiSessionData>;
    /// Creates or replaces the session entry.
    ///
    /// Returns an error if the data could not be persisted. Callers must
    /// propagate this error — silent discard leads to message loss.
    async fn put(
        &self,
        id: &str,
        data: &XaiSessionData,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    /// Removes the session entry (no-op if absent).
    async fn delete(&self, id: &str);
    /// Returns all `(session_id, cwd)` pairs sorted by session_id.
    async fn list(&self) -> Vec<(String, String)>;
}

// ── NATS KV implementation ────────────────────────────────────────────────────

pub struct KvSessionStore {
    kv: async_nats::jetstream::kv::Store,
}

impl KvSessionStore {
    /// Opens or creates the KV bucket with the given name.
    ///
    /// `max_age` sets the TTL for session entries. Use `Duration::ZERO` to
    /// disable expiry (entries live forever). The default in production is 7
    /// days (`XAI_SESSION_TTL_SECS` env var).
    pub async fn open(
        js: async_nats::jetstream::Context,
        bucket: impl Into<String>,
        max_age: std::time::Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let bucket = bucket.into();
        let config = async_nats::jetstream::kv::Config {
            bucket: bucket.clone(),
            history: 1,
            max_age,
            ..Default::default()
        };
        let kv = match js.create_key_value(config).await {
            Ok(store) => store,
            Err(_) => js.get_key_value(&bucket).await?,
        };
        Ok(Self { kv })
    }
}

#[async_trait(?Send)]
impl SessionStore for KvSessionStore {
    async fn get(&self, id: &str) -> Option<XaiSessionData> {
        match self.kv.get(id).await {
            Ok(Some(bytes)) => match serde_json::from_slice(&bytes) {
                Ok(data) => Some(data),
                Err(e) => {
                    warn!(error = %e, id, "xai: failed to deserialize session");
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                warn!(error = %e, id, "xai: KV get error");
                None
            }
        }
    }

    async fn put(
        &self,
        id: &str,
        data: &XaiSessionData,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let bytes = serde_json::to_vec(data)?;
        self.kv.put(id, bytes.into()).await?;
        Ok(())
    }

    async fn delete(&self, id: &str) {
        if let Err(e) = self.kv.delete(id).await {
            warn!(error = %e, id, "xai: KV delete error");
        }
    }

    async fn list(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        match self.kv.keys().await {
            Ok(mut keys) => {
                while let Some(item) = keys.next().await {
                    match item {
                        Ok(key) => {
                            if let Some(data) = self.get(&key).await {
                                result.push((key, data.cwd));
                            }
                        }
                        Err(e) => warn!(error = %e, "xai: KV keys error"),
                    }
                }
            }
            Err(e) => warn!(error = %e, "xai: failed to list session keys"),
        }
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }
}

// ── In-memory implementation (for testing) ───────────────────────────────────

#[cfg(feature = "test-helpers")]
pub struct MemorySessionStore {
    map: tokio::sync::Mutex<std::collections::HashMap<String, XaiSessionData>>,
}

#[cfg(feature = "test-helpers")]
impl MemorySessionStore {
    pub fn new() -> Self {
        Self { map: tokio::sync::Mutex::new(std::collections::HashMap::new()) }
    }
}

#[cfg(feature = "test-helpers")]
#[async_trait(?Send)]
impl SessionStore for MemorySessionStore {
    async fn get(&self, id: &str) -> Option<XaiSessionData> {
        self.map.lock().await.get(id).cloned()
    }

    async fn put(
        &self,
        id: &str,
        data: &XaiSessionData,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.map.lock().await.insert(id.to_string(), data.clone());
        Ok(())
    }

    async fn delete(&self, id: &str) {
        self.map.lock().await.remove(id);
    }

    async fn list(&self) -> Vec<(String, String)> {
        let map = self.map.lock().await;
        let mut result: Vec<_> =
            map.iter().map(|(k, v)| (k.clone(), v.cwd.clone())).collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }
}
