use async_nats::jetstream::Context as JsContext;
use async_nats::jetstream::kv::{Config as KvConfig, Store};
use serde::{Deserialize, Serialize};

/// Name of the JetStream KV bucket that stores conversation histories.
pub const KV_BUCKET: &str = "slack-conversations";

/// A single turn in a conversation, compatible with the Anthropic Messages API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    /// Either `"user"` or `"assistant"`.
    pub role: String,
    pub content: String,
}

/// Persistent conversation memory backed by NATS JetStream KV.
#[derive(Clone)]
pub struct ConversationMemory {
    store: Store,
    max_history: usize,
}

impl ConversationMemory {
    /// Create (or open) the KV bucket and return a `ConversationMemory`.
    pub async fn new(js: &JsContext, max_history: usize) -> Result<Self, async_nats::Error> {
        let store = js
            .create_key_value(KvConfig {
                bucket: KV_BUCKET.to_string(),
                ..Default::default()
            })
            .await?;
        Ok(Self { store, max_history })
    }

    /// Load the conversation history for `session_key`.
    /// Returns an empty Vec if the key has no history yet.
    pub async fn load(&self, session_key: &str) -> Vec<ConversationMessage> {
        let key = sanitize_key(session_key);
        match self.store.get(&key).await {
            Ok(Some(bytes)) => {
                serde_json::from_slice::<Vec<ConversationMessage>>(&bytes).unwrap_or_default()
            }
            Ok(None) => vec![],
            Err(e) => {
                tracing::warn!(error = %e, session_key, "Failed to load conversation history");
                vec![]
            }
        }
    }

    /// Persist `messages`, trimming to the last `max_history` entries.
    pub async fn save(&self, session_key: &str, messages: &[ConversationMessage]) {
        let key = sanitize_key(session_key);
        let trimmed: &[ConversationMessage] = if messages.len() > self.max_history {
            &messages[messages.len() - self.max_history..]
        } else {
            messages
        };
        match serde_json::to_vec(trimmed) {
            Ok(bytes) => {
                if let Err(e) = self.store.put(&key, bytes.into()).await {
                    tracing::warn!(error = %e, session_key, "Failed to save conversation history");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to serialize conversation history");
            }
        }
    }

    /// Delete all conversation history for `session_key`.
    pub async fn clear(&self, session_key: &str) {
        let key = sanitize_key(session_key);
        if let Err(e) = self.store.delete(&key).await {
            tracing::warn!(error = %e, session_key, "Failed to clear conversation history");
        }
    }
}

/// NATS KV keys allow `[-\w.]` — replace `:` with `.`.
fn sanitize_key(session_key: &str) -> String {
    session_key.replace(':', ".")
}
