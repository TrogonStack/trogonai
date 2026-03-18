use async_nats::jetstream;
use serde::{Deserialize, Serialize};
use trogon_agent::agent_loop::Message;

const BUCKET: &str = "ACP_SESSIONS";

/// Persisted state for a single ACP session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub messages: Vec<Message>,
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

    /// Persist updated session history.
    pub async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(state)?;
        self.kv.put(session_id, bytes.into()).await?;
        Ok(())
    }
}
