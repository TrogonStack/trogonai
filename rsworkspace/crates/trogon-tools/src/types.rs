use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── PermissionChecker ─────────────────────────────────────────────────────────

/// Called before each tool execution. Returns `true` to allow, `false` to deny.
pub trait PermissionChecker: Send + Sync {
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>>;
}

// ── ElicitationProvider ───────────────────────────────────────────────────────

/// Called when the model uses the built-in `ask_user` tool.
/// Returns the user's answer, or `None` if the user declined or cancelled.
pub trait ElicitationProvider: Send + Sync {
    fn elicit<'a>(
        &'a self,
        question: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>;
}

// ── Message types ─────────────────────────────────────────────────────────────

/// A single message in a conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

impl Message {
    #[cfg_attr(coverage, coverage(off))]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
        }
    }

    pub fn tool_results(results: Vec<ToolResult>) -> Self {
        Self {
            role: "user".to_string(),
            content: results
                .into_iter()
                .map(|r| ContentBlock::ToolResult {
                    tool_use_id: r.tool_use_id,
                    content: r.content,
                })
                .collect(),
        }
    }
}

/// Source for an image content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

/// A single block within a message's `content` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Image { source: ImageSource },
    Thinking { thinking: String },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// Pair of tool-use ID and the string result to feed back to the model.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ImageSource ───────────────────────────────────────────────────────────

    #[test]
    fn image_source_base64_roundtrip() {
        let src = ImageSource::Base64 {
            media_type: "image/png".to_string(),
            data: "abc123".to_string(),
        };
        let json = serde_json::to_string(&src).unwrap();
        let back: ImageSource = serde_json::from_str(&json).unwrap();
        match back {
            ImageSource::Base64 { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "abc123");
            }
            _ => panic!("wrong variant after roundtrip"),
        }
    }

    #[test]
    fn image_source_url_roundtrip() {
        let src = ImageSource::Url {
            url: "https://example.com/img.png".to_string(),
        };
        let json = serde_json::to_string(&src).unwrap();
        let back: ImageSource = serde_json::from_str(&json).unwrap();
        match back {
            ImageSource::Url { url } => {
                assert_eq!(url, "https://example.com/img.png");
            }
            _ => panic!("wrong variant after roundtrip"),
        }
    }

    #[test]
    fn image_source_base64_serializes_with_type_tag() {
        let src = ImageSource::Base64 {
            media_type: "image/jpeg".to_string(),
            data: "data".to_string(),
        };
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains("\"type\":\"base64\""), "must have type tag: {json}");
    }

    #[test]
    fn image_source_url_serializes_with_type_tag() {
        let src = ImageSource::Url { url: "http://x".to_string() };
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains("\"type\":\"url\""), "must have type tag: {json}");
    }

    // ── ToolResult ────────────────────────────────────────────────────────────

    #[test]
    fn tool_result_fields_are_accessible() {
        let r = ToolResult {
            tool_use_id: "call-abc".to_string(),
            content: "the result".to_string(),
        };
        assert_eq!(r.tool_use_id, "call-abc");
        assert_eq!(r.content, "the result");
    }

    #[test]
    fn tool_result_clone_is_independent() {
        let r = ToolResult {
            tool_use_id: "id".to_string(),
            content: "content".to_string(),
        };
        let mut cloned = r.clone();
        cloned.content = "changed".to_string();
        assert_eq!(r.content, "content", "clone must not mutate original");
    }

    #[test]
    fn tool_result_debug_is_non_empty() {
        let r = ToolResult {
            tool_use_id: "id".to_string(),
            content: "c".to_string(),
        };
        assert!(!format!("{r:?}").is_empty());
    }

    // ── ContentBlock::Image ───────────────────────────────────────────────────

    #[test]
    fn content_block_image_roundtrip() {
        let block = ContentBlock::Image {
            source: ImageSource::Url { url: "https://img.example.com/photo.jpg".to_string() },
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            ContentBlock::Image { source: ImageSource::Url { url } } => {
                assert_eq!(url, "https://img.example.com/photo.jpg");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn content_block_image_serializes_with_type_tag() {
        let block = ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/gif".to_string(),
                data: "R0lGODlh".to_string(),
            },
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"image\""), "ContentBlock must have type tag: {json}");
    }

    // ── ContentBlock::Thinking ────────────────────────────────────────────────

    #[test]
    fn content_block_thinking_roundtrip() {
        let block = ContentBlock::Thinking {
            thinking: "step-by-step reasoning".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            ContentBlock::Thinking { thinking } => {
                assert_eq!(thinking, "step-by-step reasoning");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn content_block_thinking_serializes_with_type_tag() {
        let block = ContentBlock::Thinking { thinking: "t".to_string() };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"thinking\""), "must have type tag: {json}");
    }

    // ── ContentBlock::ToolUse ─────────────────────────────────────────────────

    #[test]
    fn content_block_tool_use_roundtrip_without_parent() {
        let block = ContentBlock::ToolUse {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "ls"}),
            parent_tool_use_id: None,
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            ContentBlock::ToolUse { id, name, input, parent_tool_use_id } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "ls");
                assert!(parent_tool_use_id.is_none());
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn content_block_tool_use_roundtrip_with_parent() {
        let block = ContentBlock::ToolUse {
            id: "call-2".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/tmp/x"}),
            parent_tool_use_id: Some("call-1".to_string()),
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            ContentBlock::ToolUse { parent_tool_use_id, .. } => {
                assert_eq!(parent_tool_use_id.as_deref(), Some("call-1"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn content_block_tool_use_omits_parent_when_none() {
        let block = ContentBlock::ToolUse {
            id: "c".to_string(),
            name: "n".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(
            !json.contains("parent_tool_use_id"),
            "None parent_tool_use_id must be omitted from JSON: {json}"
        );
    }

    #[test]
    fn content_block_tool_use_serializes_with_type_tag() {
        let block = ContentBlock::ToolUse {
            id: "c".to_string(),
            name: "n".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"tool_use\""), "must have type tag: {json}");
    }

    // ── ContentBlock::ToolResult ──────────────────────────────────────────────

    #[test]
    fn content_block_tool_result_roundtrip() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "call-1".to_string(),
            content: "command output".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            ContentBlock::ToolResult { tool_use_id, content } => {
                assert_eq!(tool_use_id, "call-1");
                assert_eq!(content, "command output");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn content_block_tool_result_serializes_with_type_tag() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "c".to_string(),
            content: "out".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"tool_result\""), "must have type tag: {json}");
    }

    // ── Message::tool_results ─────────────────────────────────────────────────

    #[test]
    fn message_tool_results_builds_user_message_with_tool_result_blocks() {
        let results = vec![
            ToolResult { tool_use_id: "c1".to_string(), content: "out1".to_string() },
            ToolResult { tool_use_id: "c2".to_string(), content: "out2".to_string() },
        ];
        let msg = Message::tool_results(results);
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.len(), 2);
        match &msg.content[0] {
            ContentBlock::ToolResult { tool_use_id, content } => {
                assert_eq!(tool_use_id, "c1");
                assert_eq!(content, "out1");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}
