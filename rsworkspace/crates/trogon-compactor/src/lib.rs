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
pub mod traits;
pub mod types;

pub use detector::CompactionSettings;
pub use error::CompactorError;
pub use summarizer::{AnthropicLlmProvider, AuthStyle, LlmConfig};
pub use traits::LlmProvider;
pub use types::{ContentBlock, Message};

/// Convenience constructor that injects a pre-built [`reqwest::Client`].
/// Primarily intended for integration tests that point the client at a mock server.
pub fn compactor_with_client(
    config: CompactorConfig,
    client: reqwest::Client,
) -> Compactor<AnthropicLlmProvider> {
    Compactor::with_client(config, client)
}

use tracing::info;

// ── Public API ────────────────────────────────────────────────────────────────

/// Configuration for a [`Compactor`] instance.
#[derive(Default)]
pub struct CompactorConfig {
    pub settings: CompactionSettings,
    pub llm: LlmConfig,
}

/// Compacts long conversation histories by summarizing the oldest messages.
pub struct Compactor<L: LlmProvider> {
    settings: CompactionSettings,
    provider: L,
}

impl<L: LlmProvider> Compactor<L> {
    pub fn with_provider(settings: CompactionSettings, provider: L) -> Self {
        Self { settings, provider }
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

        let Some(cut_point) = detector::find_cut_point(&messages, self.settings.keep_recent_tokens)
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

        let summary = self
            .provider
            .generate_summary(messages_for_summary, previous_summary)
            .await?;

        Ok(build_compacted_history(summary, to_keep))
    }
}

impl Compactor<AnthropicLlmProvider> {
    pub fn new(config: CompactorConfig) -> Self {
        Self {
            settings: config.settings,
            provider: AnthropicLlmProvider::new(config.llm),
        }
    }

    /// Like [`new`] but accepts a pre-built [`reqwest::Client`].
    ///
    /// Useful in integration tests where the client is configured to point at a mock server.
    pub fn with_client(config: CompactorConfig, client: reqwest::Client) -> Self {
        Self {
            settings: config.settings,
            provider: AnthropicLlmProvider::with_client(config.llm, client),
        }
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
        if let ContentBlock::Text { text } = block
            && let Some(inner) = text
                .strip_prefix("<context-summary>\n")
                .and_then(|s| s.strip_suffix("\n</context-summary>"))
        {
            return Some(inner);
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

    // ── MockLlmProvider ─────────────────────────────────────────────────────

    struct MockLlmProvider {
        summary: String,
    }

    impl LlmProvider for MockLlmProvider {
        fn generate_summary<'a>(
            &'a self,
            _messages: &'a [Message],
            _previous_summary: Option<&'a str>,
        ) -> impl std::future::Future<Output = Result<String, CompactorError>> + Send + 'a {
            let s = self.summary.clone();
            async move { Ok(s) }
        }
    }

    fn msgs(pairs: &[(&str, &str)]) -> Vec<Message> {
        pairs
            .iter()
            .flat_map(|(role, text)| {
                [Message {
                    role: role.to_string(),
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
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
        let compactor = Compactor::with_provider(
            CompactionSettings::default(),
            MockLlmProvider {
                summary: String::new(),
            },
        );
        let messages = msgs(&[("user", "hi"), ("assistant", "hello")]);
        let result = compactor.compact_if_needed(messages.clone()).await.unwrap();
        assert_eq!(result.len(), messages.len());
    }

    #[tokio::test]
    async fn compact_if_needed_calls_provider_and_builds_compacted_history() {
        let settings = CompactionSettings {
            context_window: 100,
            reserve_tokens: 0,
            keep_recent_tokens: 10,
        };
        let compactor = Compactor::with_provider(
            settings,
            MockLlmProvider {
                summary: "## Goal\nTest summary".into(),
            },
        );
        // Build a conversation large enough to trigger compaction.
        let big = "x".repeat(200);
        let messages = vec![
            Message::user(format!("q1 {big}")),
            Message::assistant(format!("a1 {big}")),
            Message::user(format!("q2 {big}")),
            Message::assistant(format!("a2 {big}")),
        ];
        let result = compactor.compact_if_needed(messages).await.unwrap();
        assert_eq!(result[0].role, "user");
        let ContentBlock::Text { text } = &result[0].content[0] else {
            panic!("expected text block");
        };
        assert!(text.contains("<context-summary>"));
        assert!(text.contains("## Goal"));
        assert_eq!(result[1].role, "assistant");
    }

    #[tokio::test]
    async fn returns_unchanged_when_should_compact_but_no_actionable_cut() {
        // Triggers should_compact (total > threshold) but find_cut_point returns None
        // because the only valid user turn is at index 0.
        let settings = CompactionSettings {
            context_window: 100,
            reserve_tokens: 0,
            keep_recent_tokens: 50,
        };
        let compactor = Compactor::with_provider(
            settings,
            MockLlmProvider {
                summary: String::new(),
            }, // must not be called
        );

        // [0] real user turn (only valid cut candidate → j==0 → None)
        // [1] assistant (big)
        // [2] tool-result user (big, not a valid cut)
        let messages = vec![
            Message::user("x".repeat(200)),
            Message::assistant("x".repeat(200)),
            Message {
                role: "user".into(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "id".into(),
                    content: "x".repeat(200),
                }],
            },
        ];

        let original_len = messages.len();
        let result = compactor.compact_if_needed(messages).await.unwrap();
        assert_eq!(result.len(), original_len);
    }

    // ── build_compacted_history ─────────────────────────────────────────────

    #[test]
    fn build_compacted_history_with_empty_to_keep() {
        // No recent messages to preserve → result is just [summary_user, ack_assistant]
        let result = build_compacted_history("## Goal\nJust the summary".into(), &[]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        let ContentBlock::Text { text } = &result[0].content[0] else {
            panic!("expected text block");
        };
        assert!(text.contains("## Goal\nJust the summary"));
    }

    // ── extract_previous_summary edge cases ─────────────────────────────────

    #[test]
    fn extract_previous_summary_returns_none_for_assistant_first() {
        let messages = vec![Message::assistant(
            "<context-summary>\n## Goal\nDo stuff\n</context-summary>",
        )];
        assert!(extract_previous_summary(&messages).is_none());
    }

    #[test]
    fn extract_previous_summary_returns_none_for_partial_open_tag() {
        // Missing closing tag → strip_suffix fails → None
        let messages = vec![Message::user("<context-summary>\n## Goal\nDo stuff")];
        assert!(extract_previous_summary(&messages).is_none());
    }

    #[test]
    fn extract_previous_summary_scans_all_text_blocks_in_first_message() {
        // The function iterates all content blocks — summary in 2nd block is still found
        let messages = vec![Message {
            role: "user".into(),
            content: vec![
                ContentBlock::Text {
                    text: "regular intro".into(),
                },
                ContentBlock::Text {
                    text: "<context-summary>\n## Goal\nDo stuff\n</context-summary>".into(),
                },
            ],
        }];
        let summary = extract_previous_summary(&messages);
        assert!(summary.is_some());
        assert_eq!(summary.unwrap(), "## Goal\nDo stuff");
    }
}
