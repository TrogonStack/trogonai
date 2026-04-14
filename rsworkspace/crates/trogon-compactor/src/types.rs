//! Wire types for the compactor.
//!
//! [`Message`] and [`ContentBlock`] deliberately mirror the serde representation
//! used by `trogon-agent-core` so that JSON round-trips through NATS KV work
//! without a conversion layer.  At the integration boundary a simple `From` impl
//! handles the conversion; the compactor itself has no dependency on that crate.

use serde::{Deserialize, Serialize};

/// A single turn in an Anthropic conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// Returns `true` if this is a user message whose content is *only*
    /// `ToolResult` blocks (i.e. not a real user turn boundary).
    pub fn is_tool_result_only(&self) -> bool {
        self.role == "user"
            && !self.content.is_empty()
            && self
                .content
                .iter()
                .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
    }
}

/// A single block within a message's `content` array.
///
/// The serde tagging (`"type"` field, snake_case) matches `trogon-agent-core`
/// exactly so that stored JSON deserializes correctly here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text from the user or the model.
    Text { text: String },
    /// Image sent by the user — carried opaquely; never inspected.
    Image { source: serde_json::Value },
    /// Extended thinking block (requires thinking beta).
    Thinking { thinking: String },
    /// Tool invocation requested by the model.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    /// Result returned to the model after executing a tool.
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

impl ContentBlock {
    /// Best-effort textual content, used for token estimation and serialization.
    /// Returns `None` for blocks (like images) whose content cannot be extracted.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Thinking { thinking } => Some(thinking),
            Self::ToolUse { name, .. } => Some(name.as_str()),
            Self::ToolResult { content, .. } => Some(content),
            Self::Image { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_tool_result_only ────────────────────────────────────────────────────

    #[test]
    fn is_tool_result_only_true_for_single_tool_result() {
        let msg = Message {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "id".into(),
                content: "result".into(),
            }],
        };
        assert!(msg.is_tool_result_only());
    }

    #[test]
    fn is_tool_result_only_false_for_empty_content() {
        let msg = Message { role: "user".into(), content: vec![] };
        assert!(!msg.is_tool_result_only());
    }

    #[test]
    fn is_tool_result_only_false_for_mixed_content() {
        let msg = Message {
            role: "user".into(),
            content: vec![
                ContentBlock::Text { text: "hello".into() },
                ContentBlock::ToolResult {
                    tool_use_id: "id".into(),
                    content: "result".into(),
                },
            ],
        };
        assert!(!msg.is_tool_result_only());
    }

    #[test]
    fn is_tool_result_only_false_for_assistant_role() {
        let msg = Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "id".into(),
                content: "result".into(),
            }],
        };
        assert!(!msg.is_tool_result_only());
    }

    // ── ContentBlock::as_text ─────────────────────────────────────────────────

    #[test]
    fn as_text_returns_text_content() {
        let b = ContentBlock::Text { text: "hello".into() };
        assert_eq!(b.as_text(), Some("hello"));
    }

    #[test]
    fn as_text_returns_thinking_text() {
        let b = ContentBlock::Thinking { thinking: "deep thought".into() };
        assert_eq!(b.as_text(), Some("deep thought"));
    }

    #[test]
    fn as_text_returns_tool_use_name() {
        let b = ContentBlock::ToolUse {
            id: "1".into(),
            name: "my_tool".into(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        };
        assert_eq!(b.as_text(), Some("my_tool"));
    }

    #[test]
    fn as_text_returns_tool_result_content() {
        let b = ContentBlock::ToolResult {
            tool_use_id: "1".into(),
            content: "the output".into(),
        };
        assert_eq!(b.as_text(), Some("the output"));
    }

    #[test]
    fn as_text_returns_none_for_image() {
        let b = ContentBlock::Image { source: serde_json::json!({"type": "base64"}) };
        assert!(b.as_text().is_none());
    }
}
