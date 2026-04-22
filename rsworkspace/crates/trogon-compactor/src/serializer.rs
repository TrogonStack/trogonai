//! Converts a message slice into a readable text representation for the
//! summarization prompt.  Tool result content is truncated to keep the prompt
//! manageable.

use crate::types::{ContentBlock, Message};

const TOOL_RESULT_TRUNCATE: usize = 2_000;

/// Serializes `messages` into a human-readable text block suitable for
/// inclusion in a summarization prompt.
pub fn serialize_for_prompt(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(messages.len());

    for msg in messages {
        match msg.role.as_str() {
            "user" => serialize_user_message(msg, &mut parts),
            "assistant" => serialize_assistant_message(msg, &mut parts),
            _ => {}
        }
    }

    parts.join("\n\n")
}

fn serialize_user_message(msg: &Message, out: &mut Vec<String>) {
    if msg.is_tool_result_only() {
        for block in &msg.content {
            if let ContentBlock::ToolResult { content, .. } = block {
                let truncated = truncate(content, TOOL_RESULT_TRUNCATE);
                out.push(format!("[Tool result]: {truncated}"));
            }
        }
    } else {
        let text: String = msg
            .content
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            out.push(format!("[User]: {text}"));
        }
    }
}

fn serialize_assistant_message(msg: &Message, out: &mut Vec<String>) {
    let mut text_parts: Vec<&str> = Vec::new();
    let mut tool_calls: Vec<String> = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => text_parts.push(text),
            ContentBlock::Thinking { thinking } => {
                out.push(format!("[Assistant thinking]: {thinking}"));
            }
            ContentBlock::ToolUse { name, input, .. } => {
                tool_calls.push(format!("{name}({input})"));
            }
            _ => {}
        }
    }

    if !text_parts.is_empty() {
        out.push(format!("[Assistant]: {}", text_parts.join("\n")));
    }
    if !tool_calls.is_empty() {
        out.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
    }
}

fn truncate(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        s.to_owned()
    } else {
        format!(
            "{}…\n[{} more chars truncated]",
            &s[..max_bytes],
            s.len() - max_bytes
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_user_and_assistant_turns() {
        let msgs = vec![
            Message::user("What is 2+2?"),
            Message::assistant("It is 4."),
        ];
        let out = serialize_for_prompt(&msgs);
        assert!(out.contains("[User]: What is 2+2?"));
        assert!(out.contains("[Assistant]: It is 4."));
    }

    #[test]
    fn tool_result_truncated_when_long() {
        let long_content = "x".repeat(TOOL_RESULT_TRUNCATE + 100);
        let msg = Message {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "id1".into(),
                content: long_content,
            }],
        };
        let out = serialize_for_prompt(&[msg]);
        assert!(out.contains("truncated"));
        assert!(out.starts_with("[Tool result]:"));
    }

    #[test]
    fn tool_use_block_becomes_tool_calls_line() {
        let msg = Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "/tmp/foo.txt"}),
                parent_tool_use_id: None,
            }],
        };
        let out = serialize_for_prompt(&[msg]);
        assert!(out.contains("[Assistant tool calls]: read_file("));
    }

    #[test]
    fn empty_messages_returns_empty_string() {
        assert_eq!(serialize_for_prompt(&[]), "");
    }

    #[test]
    fn thinking_block_in_assistant_is_serialized() {
        let msg = Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Thinking {
                thinking: "deep thought".into(),
            }],
        };
        let out = serialize_for_prompt(&[msg]);
        assert!(out.contains("[Assistant thinking]: deep thought"));
    }

    #[test]
    fn short_tool_result_is_not_truncated() {
        let msg = Message {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "id1".into(),
                content: "short output".into(),
            }],
        };
        let out = serialize_for_prompt(&[msg]);
        assert_eq!(out, "[Tool result]: short output");
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn unknown_role_is_silently_skipped() {
        let msg = Message {
            role: "system".into(),
            content: vec![ContentBlock::Text {
                text: "secret system prompt".into(),
            }],
        };
        let out = serialize_for_prompt(&[msg]);
        assert!(out.is_empty());
    }

    #[test]
    fn image_block_in_assistant_produces_no_output() {
        // Image blocks in assistant messages are silently skipped
        let msg = Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Image {
                source: serde_json::json!({"type": "base64", "media_type": "image/png"}),
            }],
        };
        let out = serialize_for_prompt(&[msg]);
        assert!(out.is_empty());
    }

    #[test]
    fn user_message_with_only_empty_text_produces_no_output() {
        // Empty text after joining is not emitted
        let msg = Message {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: "".into() }],
        };
        let out = serialize_for_prompt(&[msg]);
        assert!(out.is_empty());
    }

    #[test]
    fn tool_result_at_exact_truncation_boundary_is_not_truncated() {
        // s.len() == max_bytes → the ≤ branch → no truncation
        let content = "x".repeat(TOOL_RESULT_TRUNCATE);
        let msg = Message {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "id1".into(),
                content,
            }],
        };
        let out = serialize_for_prompt(&[msg]);
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn multiple_tool_calls_are_joined_with_semicolon() {
        let msg = Message {
            role: "assistant".into(),
            content: vec![
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({}),
                    parent_tool_use_id: None,
                },
                ContentBlock::ToolUse {
                    id: "t2".into(),
                    name: "write_file".into(),
                    input: serde_json::json!({}),
                    parent_tool_use_id: None,
                },
            ],
        };
        let out = serialize_for_prompt(&[msg]);
        assert!(out.contains("read_file("));
        assert!(out.contains("write_file("));
        assert!(out.contains("; "));
    }
}
