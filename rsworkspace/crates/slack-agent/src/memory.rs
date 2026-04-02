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
    /// Maximum total character count of message content to retain.
    /// `0` means disabled. Characters are used as a token proxy (chars ÷ 4 ≈ tokens).
    max_chars: usize,
}

impl ConversationMemory {
    /// Create (or open) the KV bucket and return a `ConversationMemory`.
    ///
    /// `max_chars` caps total content character count; pass `0` to disable.
    pub async fn new(
        js: &JsContext,
        max_history: usize,
        max_chars: usize,
    ) -> Result<Self, async_nats::Error> {
        let store = js
            .create_key_value(KvConfig {
                bucket: KV_BUCKET.to_string(),
                // Expire entries after 30 days of inactivity to prevent unbounded growth.
                max_age: Duration::from_secs(30 * 24 * 3600),
                ..Default::default()
            })
            .await?;
        Ok(Self {
            store,
            max_history,
            max_chars,
        })
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

    /// Persist `messages`, trimming to the last `max_history` entries and then
    /// by total content character count when `max_chars > 0`.
    pub async fn save(&self, session_key: &str, messages: &[ConversationMessage]) {
        let key = sanitize_key(session_key);
        // First pass: count-based trim.
        let after_count: &[ConversationMessage] = if messages.len() > self.max_history {
            &messages[messages.len() - self.max_history..]
        } else {
            messages
        };
        // Second pass: character-based trim.
        let trimmed = trim_by_chars(after_count, self.max_chars);
        if trimmed.len() < after_count.len() {
            tracing::debug!(
                session_key,
                dropped = after_count.len() - trimmed.len(),
                remaining = trimmed.len(),
                max_chars = self.max_chars,
                "Truncated conversation history by character limit"
            );
        }
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

/// Trim `messages` from the front so that the total `content.len()` of all
/// remaining messages is at most `max_chars`. Always retains at least the last
/// 2 messages to avoid dropping everything. Returns the trimmed sub-slice.
///
/// When `max_chars == 0` the input slice is returned unchanged.
fn trim_by_chars<'a>(
    messages: &'a [ConversationMessage],
    max_chars: usize,
) -> &'a [ConversationMessage] {
    if max_chars == 0 || messages.is_empty() {
        return messages;
    }
    // Minimum messages to always keep, regardless of size.
    const MIN_KEEP: usize = 2;

    // Accumulate character counts from the END of the slice and find the
    // earliest index we can include without exceeding max_chars.
    let mut total = 0usize;
    // Start by keeping nothing; we expand towards the front in each iteration.
    let mut keep_from = messages.len();

    for (i, msg) in messages.iter().enumerate().rev() {
        let new_total = total.saturating_add(msg.content.len());
        let would_keep = messages.len() - i; // messages kept if we include index i
        if new_total > max_chars && would_keep > MIN_KEEP {
            // Adding this message would exceed the limit and we already have
            // at least MIN_KEEP messages — stop and do not include this one.
            break;
        }
        total = new_total;
        keep_from = i;
    }

    &messages[keep_from..]
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

    // ── trim_by_chars tests ───────────────────────────────────────────────────

    fn make_msg(role: &str, content: &str) -> ConversationMessage {
        ConversationMessage {
            role: role.to_string(),
            content: content.to_string(),
            ts: None,
            images: vec![],
        }
    }

    #[test]
    fn trim_by_chars_zero_disabled() {
        let msgs = vec![
            make_msg("user", "hello world"),
            make_msg("assistant", "hi there"),
        ];
        // max_chars = 0 means disabled — nothing is trimmed.
        let result = trim_by_chars(&msgs, 0);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn trim_by_chars_empty_slice() {
        let msgs: Vec<ConversationMessage> = vec![];
        let result = trim_by_chars(&msgs, 10);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn trim_by_chars_within_limit_unchanged() {
        let msgs = vec![
            make_msg("user", "hi"),      // 2 chars
            make_msg("assistant", "ok"), // 2 chars
        ];
        // total = 4, limit = 100 — nothing dropped
        let result = trim_by_chars(&msgs, 100);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn trim_by_chars_drops_front_messages_when_over_limit() {
        let msgs = vec![
            make_msg("user", "aaaa"),      // 4 chars — should be dropped
            make_msg("assistant", "bbbb"), // 4 chars — should be dropped
            make_msg("user", "cccc"),      // 4 chars
            make_msg("assistant", "dddd"), // 4 chars
        ];
        // limit = 8: only last 2 messages fit
        let result = trim_by_chars(&msgs, 8);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "cccc");
        assert_eq!(result[1].content, "dddd");
    }

    #[test]
    fn trim_by_chars_keeps_at_least_two_messages() {
        let msgs = vec![
            make_msg("user", "a very long message that exceeds the limit"),
            make_msg("assistant", "another very long response that also exceeds"),
        ];
        // limit = 1 — would drop everything, but MIN_KEEP = 2 applies
        let result = trim_by_chars(&msgs, 1);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn trim_by_chars_single_message_always_kept() {
        let msgs = vec![make_msg("user", "hello world")];
        // limit = 1 — single message, MIN_KEEP = 2 but only 1 exists
        let result = trim_by_chars(&msgs, 1);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn trim_by_chars_exactly_at_limit_unchanged() {
        let msgs = vec![
            make_msg("user", "abcd"),      // 4 chars
            make_msg("assistant", "efgh"), // 4 chars
        ];
        // total = 8, limit = 8 — exactly at limit, nothing dropped
        let result = trim_by_chars(&msgs, 8);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn trim_by_chars_drops_multiple_from_front() {
        // 5 messages, each with 10-char content. Total = 50 chars.
        // With limit = 25, only last 2 messages (20 chars) fit if we're strict,
        // but last 3 (30 chars) don't fit — so we keep 2.
        let msgs = vec![
            make_msg("user", "0123456789"),
            make_msg("assistant", "abcdefghij"),
            make_msg("user", "ABCDEFGHIJ"),
            make_msg("assistant", "KLMNOPQRST"),
            make_msg("user", "zyxwvutsrq"),
        ];
        let result = trim_by_chars(&msgs, 25);
        // Last 2 messages = 20 chars (<= 25). Last 3 messages = 30 chars (> 25).
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "KLMNOPQRST");
        assert_eq!(result[1].content, "zyxwvutsrq");
    }
}
