use async_nats::jetstream;
use serde::{Deserialize, Serialize};
use trogon_agent::agent_loop::Message;

const BUCKET: &str = "ACP_SESSIONS";

/// Persisted state for a single ACP session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub messages: Vec<Message>,
    /// Per-session model override. `None` means use the agent's default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Permission mode (e.g. "default", "acceptEdits", "bypassPermissions").
    #[serde(default)]
    pub mode: String,
    /// Working directory recorded when the session was created.
    #[serde(default)]
    pub cwd: String,
    /// ISO-8601 creation timestamp.
    #[serde(default)]
    pub created_at: String,
}

/// NATS KV-backed session store.
pub struct SessionStore {
    kv: jetstream::kv::Store,
}

impl SessionStore {
    /// Create or open the `ACP_SESSIONS` KV bucket.
    pub async fn open(js: &jetstream::Context) -> anyhow::Result<Self> {
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket: BUCKET.to_string(),
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }

    /// Load session history, returning an empty state if the key does not exist.
    pub async fn load(&self, session_id: &str) -> anyhow::Result<SessionState> {
        match self.kv.get(session_id).await? {
            Some(bytes) => Ok(serde_json::from_slice(&bytes)?),
            None => Ok(SessionState::default()),
        }
    }

    /// Persist updated session state.
    pub async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(state)?;
        self.kv.put(session_id, bytes.into()).await?;
        Ok(())
    }

    /// Delete a session from the store (best-effort).
    pub async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
        self.kv.delete(session_id).await?;
        Ok(())
    }

    /// List all session IDs currently in the store.
    pub async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
        use futures_util::StreamExt;
        let mut keys = self.kv.keys().await?;
        let mut ids = Vec::new();
        while let Some(key) = keys.next().await {
            match key {
                Ok(k) => ids.push(k),
                Err(e) => tracing::warn!(error = %e, "session_store: error reading key"),
            }
        }
        Ok(ids)
    }
}
