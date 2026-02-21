use serde::{Deserialize, Serialize};

// ── Subjects ──────────────────────────────────────────────────────────────────

/// Core NATS subject for Anthropic/Claude LLM requests.
pub const LLM_REQUEST_ANTHROPIC: &str = "llm.request.prompt.anthropic";

/// Core NATS subject for OpenAI LLM requests.
pub const LLM_REQUEST_OPENAI: &str = "llm.request.prompt.openai";

/// Core NATS subject for Google Gemini LLM requests.
pub const LLM_REQUEST_GEMINI: &str = "llm.request.prompt.gemini";

/// Core NATS subject for DeepSeek LLM requests.
pub const LLM_REQUEST_DEEPSEEK: &str = "llm.request.prompt.deepseek";

/// Inserts an optional account-ID namespace into an LLM subject.
/// `"llm.request.prompt.anthropic"` → `"llm.ws1.request.prompt.anthropic"`
pub fn llm_subject_for_account(subject: &str, account_id: Option<&str>) -> String {
    match account_id.filter(|id| !id.is_empty()) {
        Some(id) => {
            if let Some(rest) = subject.strip_prefix("llm.") {
                format!("llm.{id}.{rest}")
            } else {
                subject.to_string()
            }
        }
        None => subject.to_string(),
    }
}

// ── Message types ─────────────────────────────────────────────────────────────

/// A single conversation turn sent to the LLM worker.
/// `content` follows the Anthropic wire format: a plain JSON string for
/// text-only messages, or a JSON array of content blocks for multimodal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    /// Either `Value::String("…")` or `Value::Array([{type,source/text}, …])`.
    pub content: serde_json::Value,
}

// ── Request / response ────────────────────────────────────────────────────────

/// Published by an agent to `llm.request.prompt.anthropic`.
/// The worker streams `LlmStreamChunk` messages to `stream_inbox`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmPromptRequest {
    pub messages: Vec<LlmMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Model override. Worker uses its configured default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub max_tokens: u32,
    /// NATS subject where the worker publishes `LlmStreamChunk` messages.
    /// The agent subscribes to this inbox before publishing the request.
    pub stream_inbox: String,
}

/// Streaming chunk published by the worker to `LlmPromptRequest::stream_inbox`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStreamChunk {
    /// Incremental text delta (empty string on the final `done` message).
    #[serde(default)]
    pub text: String,
    /// `true` on the last message of the stream.
    pub done: bool,
    /// Full accumulated text — only present when `done = true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_text: Option<String>,
    /// Set when the worker encounters an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_subject_for_account_none_returns_bare() {
        assert_eq!(
            llm_subject_for_account(LLM_REQUEST_ANTHROPIC, None),
            LLM_REQUEST_ANTHROPIC
        );
    }

    #[test]
    fn llm_subject_for_account_empty_returns_bare() {
        assert_eq!(
            llm_subject_for_account(LLM_REQUEST_ANTHROPIC, Some("")),
            LLM_REQUEST_ANTHROPIC
        );
    }

    #[test]
    fn llm_subject_for_account_inserts_id() {
        assert_eq!(
            llm_subject_for_account(LLM_REQUEST_ANTHROPIC, Some("ws1")),
            "llm.ws1.request.prompt.anthropic"
        );
    }

    #[test]
    fn stream_chunk_roundtrip() {
        let chunk = LlmStreamChunk {
            text: "hello".into(),
            done: false,
            full_text: None,
            error: None,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        let back: LlmStreamChunk = serde_json::from_str(&json).unwrap();
        assert_eq!(back.text, "hello");
        assert!(!back.done);
    }

    #[test]
    fn stream_chunk_done_with_full_text() {
        let chunk = LlmStreamChunk {
            text: String::new(),
            done: true,
            full_text: Some("full response".into()),
            error: None,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"done\":true"));
        assert!(json.contains("full_text"));
    }

    #[test]
    fn prompt_request_roundtrip() {
        let req = LlmPromptRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: serde_json::Value::String("hi".into()),
            }],
            system: Some("Be concise".into()),
            model: Some("claude-sonnet-4-6".into()),
            max_tokens: 1024,
            stream_inbox: "_INBOX.abc123".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: LlmPromptRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stream_inbox, "_INBOX.abc123");
        assert_eq!(back.max_tokens, 1024);
        assert_eq!(back.messages[0].role, "user");
    }
}
