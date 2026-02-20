use async_nats::jetstream::Context as JsContext;
use async_nats::jetstream::kv::{Config as KvConfig, Store};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Name of the JetStream KV bucket that stores conversation histories.
pub const KV_BUCKET: &str = "slack-conversations";

/// A base64-encoded image attached to a conversation message, for Claude vision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageData {
    /// Raw image bytes encoded as standard base64.
    pub base64: String,
    /// MIME type: `"image/jpeg"`, `"image/png"`, `"image/gif"`, or `"image/webp"`.
    pub media_type: String,
}

/// A single turn in a conversation, compatible with the Anthropic Messages API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    /// Either `"user"` or `"assistant"`.
    pub role: String,
    pub content: String,
    /// Slack message timestamp — set for user turns so edited/deleted messages
    /// can be found and updated in history. Absent for assistant turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    /// Base64-encoded images for Claude vision. Only set on user turns with image
    /// attachments. Empty for assistant turns and text-only user turns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageData>,
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
                // Expire entries after 30 days of inactivity to prevent unbounded growth.
                max_age: Duration::from_secs(30 * 24 * 3600),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_key_replaces_colons() {
        assert_eq!(sanitize_key("slack:channel:C123"), "slack.channel.C123");
        assert_eq!(
            sanitize_key("slack:channel:C123:thread:1234.5"),
            "slack.channel.C123.thread.1234.5"
        );
        assert_eq!(sanitize_key("slack:dm:U456"), "slack.dm.U456");
    }

    #[test]
    fn sanitize_key_no_colons_unchanged() {
        assert_eq!(sanitize_key("simple"), "simple");
        assert_eq!(sanitize_key("slack.channel.C1"), "slack.channel.C1");
    }

    #[test]
    fn conversation_message_images_backward_compat() {
        // Old messages without images field should deserialise with empty vec.
        let json = r#"[{"role":"user","content":"hello"}]"#;
        let msgs: Vec<ConversationMessage> = serde_json::from_str(json).unwrap();
        assert!(msgs[0].images.is_empty());
    }

    #[test]
    fn conversation_message_images_roundtrip() {
        let msg = ConversationMessage {
            role: "user".to_string(),
            content: "check this image".to_string(),
            ts: None,
            images: vec![ImageData {
                base64: "abc123".to_string(),
                media_type: "image/png".to_string(),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ConversationMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.images.len(), 1);
        assert_eq!(decoded.images[0].media_type, "image/png");
    }

    #[test]
    fn image_data_without_images_omits_field() {
        // When images is empty, the field should be omitted from serialized JSON.
        let msg = ConversationMessage {
            role: "user".to_string(),
            content: "text".to_string(),
            ts: None,
            images: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("images"));
    }
}
