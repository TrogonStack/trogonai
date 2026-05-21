use serde::{Deserialize, Serialize};

/// A typed block inside a [`PortableMessage`].
///
/// Used for cross-runner session export/import so that tool-call history can be
/// faithfully reconstructed in the receiving runner's native format (Anthropic
/// ToolUse/ToolResult, OpenAI tool_calls / role:"tool", etc.).
///
/// `Thinking` blocks are intentionally absent: they are Anthropic-specific and
/// have no cross-API equivalent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PortableBlock {
    Text { text: String },
    ToolCall { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_call_id: String, content: String },
}

/// A single turn in a portable session history.
///
/// `text` is a plain-text fallback for runners that are text-only (codex,
/// xai) or for displaying history to users.  `blocks` carries the structured
/// content; when non-empty it is the authoritative source and receivers should
/// prefer it over `text`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableMessage {
    pub role: String, // "user" | "assistant"
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<PortableBlock>,
}

impl PortableMessage {
    /// Convenience constructor for text-only turns (codex, xai history).
    pub fn text_only(role: impl Into<String>, text: impl Into<String>) -> Self {
        Self { role: role.into(), text: text.into(), blocks: vec![] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_message_serde_round_trip() {
        let original = PortableMessage::text_only("user", "hello world");
        let json = serde_json::to_string(&original).unwrap();
        let decoded: PortableMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.role, original.role);
        assert_eq!(decoded.text, original.text);
        assert!(decoded.blocks.is_empty());
    }

    #[test]
    fn portable_message_vec_round_trip() {
        let original = vec![
            PortableMessage::text_only("user", "q"),
            PortableMessage::text_only("assistant", "a"),
        ];
        let json = serde_json::to_string(&original).unwrap();
        let decoded: Vec<PortableMessage> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].role, "user");
        assert_eq!(decoded[0].text, "q");
        assert_eq!(decoded[1].role, "assistant");
        assert_eq!(decoded[1].text, "a");
    }

    #[test]
    fn portable_message_json_shape() {
        let msg = PortableMessage::text_only("user", "hi");
        let json = serde_json::to_string(&msg).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["role"], "user");
        assert_eq!(value["text"], "hi");
        // blocks omitted when empty
        assert!(value.get("blocks").is_none());
    }

    #[test]
    fn cross_runner_export_json_importable_by_all_runners() {
        // Old format (no blocks field) still deserializes correctly.
        let json = r#"[{"role":"user","text":"question"},{"role":"assistant","text":"answer"}]"#;
        let decoded: Vec<PortableMessage> = serde_json::from_str(json).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].role, "user");
        assert_eq!(decoded[0].text, "question");
        assert!(decoded[0].blocks.is_empty());
        assert_eq!(decoded[1].role, "assistant");
        assert_eq!(decoded[1].text, "answer");
    }

    #[test]
    fn portable_block_serde_round_trip() {
        let blocks = vec![
            PortableBlock::Text { text: "hello".into() },
            PortableBlock::ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "/foo"}),
            },
            PortableBlock::ToolResult { tool_call_id: "c1".into(), content: "file contents".into() },
        ];
        let msg = PortableMessage { role: "assistant".into(), text: "hello".into(), blocks };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: PortableMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.blocks.len(), 3);
        match &decoded.blocks[1] {
            PortableBlock::ToolCall { id, name, input } => {
                assert_eq!(id, "c1");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/foo");
            }
            _ => panic!("expected ToolCall"),
        }
        match &decoded.blocks[2] {
            PortableBlock::ToolResult { tool_call_id, content } => {
                assert_eq!(tool_call_id, "c1");
                assert_eq!(content, "file contents");
            }
            _ => panic!("expected ToolResult"),
        }
    }
}
