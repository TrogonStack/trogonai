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
        out.push(format!(
            "[Assistant tool calls]: {}",
            tool_calls.join("; ")
        ));
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
}
