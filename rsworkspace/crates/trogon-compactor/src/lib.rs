//! Context compaction for long-running Claude conversations.
//!
//! When a conversation's estimated token count approaches the model's context
//! window limit, [`Compactor::compact_if_needed`] transparently summarizes the
//! oldest portion of the history into a structured checkpoint, keeping only the
//! most recent messages verbatim.
//!
//! # Design
//!
//! [`Message`] and [`ContentBlock`] mirror the serde representation used by
//! `trogon-agent-core` so that messages stored in NATS KV round-trip correctly
//! without a conversion layer.  When integrated, a simple `From` impl at the
//! call site handles the type boundary; this crate has no dependency on
//! `trogon-agent-core` or any other workspace crate.
//!
//! # Compaction flow
//!
//! 1. **Detect**: estimate total tokens; bail early if under the threshold.
//! 2. **Cut**: find the oldest message to keep verbatim (a real user turn
//!    boundary, never a tool-result message).
//! 3. **Summarize**: call the LLM with a structured prompt.  If a previous
//!    compaction summary exists at the front of the history, use an *update*
//!    prompt so the model merges rather than regenerates.
//! 4. **Rebuild**: `[summary_user, ack_assistant, ...kept_messages]`.

pub mod detector;
pub mod error;
pub mod serializer;
pub mod service;
pub mod summarizer;
pub mod tokens;
pub mod types;

pub use detector::CompactionSettings;
pub use error::CompactorError;
pub use summarizer::{AuthStyle, LlmConfig};
pub use types::{ContentBlock, Message};

/// Convenience constructor that injects a pre-built [`reqwest::Client`].
/// Primarily intended for tests that point the client at a mock server.
pub fn compactor_with_client(config: CompactorConfig, client: reqwest::Client) -> Compactor {
    Compactor::with_client(config, client)
}

use reqwest::Client;
use tracing::info;

// ── Public API ────────────────────────────────────────────────────────────────

/// Configuration for a [`Compactor`] instance.
pub struct CompactorConfig {
    pub settings: CompactionSettings,
    pub llm: LlmConfig,
}

impl Default for CompactorConfig {
    fn default() -> Self {
        Self {
            settings: CompactionSettings::default(),
            llm: LlmConfig::default(),
        }
    }
}

/// Compacts long conversation histories by summarizing the oldest messages.
pub struct Compactor {
    settings: CompactionSettings,
    llm: LlmConfig,
    client: Client,
}

impl Compactor {
    pub fn new(config: CompactorConfig) -> Self {
        Self {
            settings: config.settings,
            llm: config.llm,
            client: Client::new(),
        }
    }

    /// Like [`new`] but accepts a pre-built [`Client`].
    ///
    /// Useful in tests where the client is configured to point at a mock server.
    pub fn with_client(config: CompactorConfig, client: Client) -> Self {
        Self {
            settings: config.settings,
            llm: config.llm,
            client,
        }
    }

    /// Returns `messages` unchanged when compaction is not needed.
    ///
    /// When the estimated token count is close to the context window limit,
    /// the oldest messages are summarized and replaced with a compact
    /// checkpoint, preserving the most recent `keep_recent_tokens` worth of
    /// history verbatim.
    pub async fn compact_if_needed(
        &self,
        messages: Vec<Message>,
    ) -> Result<Vec<Message>, CompactorError> {
        if !detector::should_compact(&messages, &self.settings) {
            return Ok(messages);
        }

        let Some(cut_point) =
            detector::find_cut_point(&messages, self.settings.keep_recent_tokens)
        else {
            return Ok(messages);
        };

        let (to_summarize, to_keep) = messages.split_at(cut_point);

        // If the first message is already a compaction summary, extract it so
        // the LLM can update it incrementally rather than regenerate from scratch.
        let previous_summary = extract_previous_summary(to_summarize);

        // When a previous summary exists, skip it when feeding messages to the
        // summarizer (it is passed separately via `previous_summary`).
        let messages_for_summary = if previous_summary.is_some() {
            to_summarize.get(1..).unwrap_or(&[])
        } else {
            to_summarize
        };

        info!(
            cut_point,
            messages_to_summarize = messages_for_summary.len(),
            messages_to_keep = to_keep.len(),
            incremental = previous_summary.is_some(),
            "compacting conversation history",
        );

        let summary = summarizer::generate_summary(
            messages_for_summary,
            previous_summary,
            &self.llm,
            &self.client,
        )
        .await?;

        Ok(build_compacted_history(summary, to_keep))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns the summary text if the first message is a compaction checkpoint.
fn extract_previous_summary(messages: &[Message]) -> Option<&str> {
    let first = messages.first()?;
    if first.role != "user" {
        return None;
    }
    for block in &first.content {
        if let ContentBlock::Text { text } = block {
            if let Some(inner) = text
                .strip_prefix("<context-summary>\n")
                .and_then(|s| s.strip_suffix("\n</context-summary>"))
            {
                return Some(inner);
            }
        }
    }
    None
}

/// Builds `[summary_user_msg, ack_assistant_msg, ...to_keep]`.
fn build_compacted_history(summary: String, to_keep: &[Message]) -> Vec<Message> {
    let mut out = Vec::with_capacity(2 + to_keep.len());
    out.push(Message::user(format!(
        "<context-summary>\n{summary}\n</context-summary>"
    )));
    out.push(Message::assistant(
        "I've reviewed the conversation summary and will continue from this context.",
    ));
    out.extend_from_slice(to_keep);
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs(pairs: &[(&str, &str)]) -> Vec<Message> {
        pairs
            .iter()
            .flat_map(|(role, text)| {
                [Message {
                    role: role.to_string(),
                    content: vec![ContentBlock::Text { text: text.to_string() }],
                }]
            })
            .collect()
    }

    // ── extract_previous_summary ────────────────────────────────────────────

    #[test]
    fn detects_compaction_summary_in_first_message() {
        let messages = vec![Message::user(
            "<context-summary>\n## Goal\nDo the thing\n</context-summary>",
        )];
        let summary = extract_previous_summary(&messages);
        assert!(summary.is_some());
        assert!(summary.unwrap().contains("## Goal"));
    }

    #[test]
    fn returns_none_for_normal_user_message() {
        let messages = msgs(&[("user", "Hello!")]);
        assert!(extract_previous_summary(&messages).is_none());
    }

    #[test]
    fn returns_none_when_messages_empty() {
        assert!(extract_previous_summary(&[]).is_none());
    }

    // ── build_compacted_history ─────────────────────────────────────────────

    #[test]
    fn compacted_history_has_correct_structure() {
        let kept = msgs(&[("user", "last q"), ("assistant", "last a")]);
        let result = build_compacted_history("## Goal\nFinish task".into(), &kept);

        assert_eq!(result.len(), 4); // summary + ack + 2 kept
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "user");
        assert_eq!(result[3].role, "assistant");

        // First message wraps the summary in XML tags
        let ContentBlock::Text { text } = &result[0].content[0] else {
            panic!("expected text block");
        };
        assert!(text.starts_with("<context-summary>"));
        assert!(text.ends_with("</context-summary>"));
    }

    // ── compact_if_needed (no LLM) ──────────────────────────────────────────

    #[tokio::test]
    async fn returns_unchanged_when_under_threshold() {
        let compactor = Compactor::new(CompactorConfig::default());
        // Tiny conversation, nowhere near 200k tokens
        let messages = msgs(&[("user", "hi"), ("assistant", "hello")]);
        let result = compactor.compact_if_needed(messages.clone()).await.unwrap();
        assert_eq!(result.len(), messages.len());
    }
}
