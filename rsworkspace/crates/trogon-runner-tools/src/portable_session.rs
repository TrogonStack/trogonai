use serde::{Deserialize, Serialize};
use trogon_tools::{ContentBlock, Message};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
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

pub const EXPORT_VERSION_V2: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PortableBlock {
    Text { text: String },
    ToolUse {
        id: String,
        name: String,
        input_summary: String,
    },
    ToolResult {
        id: String,
        output_summary: String,
    },
    Thinking {
        text: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortableMessageV2 {
    pub version: u32,
    pub role: String,
    pub blocks: Vec<PortableBlock>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortableExportV2 {
    pub version: u32,
    pub messages: Vec<PortableMessageV2>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedExport {
    V1(Vec<PortableMessage>),
    V2(PortableExportV2),
}

fn summarize_value(value: &serde_json::Value) -> String {
    let s = value.to_string();
    truncate_str(&s, 240)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Slice on a UTF-8 char boundary: `&s[..max-1]` panics when that byte
        // index falls inside a multibyte char (e.g. tool output with accents/CJK).
        let boundary = s.floor_char_boundary(max.saturating_sub(1));
        format!("{}…", &s[..boundary])
    }
}

/// Returns `true` when any block is richer than plain text.
pub fn messages_need_v2(messages: &[Message]) -> bool {
    messages.iter().any(|m| {
        m.content.iter().any(|b| {
            !matches!(
                b,
                ContentBlock::Text { .. } | ContentBlock::Image { .. }
            )
        })
    })
}

pub fn message_to_v2(m: &Message) -> PortableMessageV2 {
    let blocks = m
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => PortableBlock::Text { text: text.clone() },
            ContentBlock::ToolUse { id, name, input, .. } => PortableBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input_summary: summarize_value(input),
            },
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => PortableBlock::ToolResult {
                id: tool_use_id.clone(),
                output_summary: truncate_str(content, 500),
            },
            ContentBlock::Thinking { thinking, .. } => PortableBlock::Thinking {
                text: thinking.clone(),
            },
            ContentBlock::Image { .. } => PortableBlock::Text {
                text: "[image]".into(),
            },
        })
        .collect();
    PortableMessageV2 {
        version: EXPORT_VERSION_V2,
        role: m.role.clone(),
        blocks,
    }
}

pub fn messages_to_export_v2(messages: &[Message]) -> PortableExportV2 {
    PortableExportV2 {
        version: EXPORT_VERSION_V2,
        messages: messages.iter().map(message_to_v2).collect(),
    }
}

/// Convert a codex-style history (`Vec<PortableMessage>` that already carries rich
/// `blocks` for tool turns) into a V2 export. Text-only messages (empty `blocks`)
/// become a single `Text` block so importers never reconstruct an empty-content
/// message — and so the export is emitted as V2 rather than a bare V1 array, which
/// every V1 importer would flatten by dropping `blocks` (turning tool results into
/// empty text and triggering an Anthropic 400).
pub fn portable_messages_to_export_v2(messages: &[PortableMessage]) -> PortableExportV2 {
    PortableExportV2 {
        version: EXPORT_VERSION_V2,
        messages: messages
            .iter()
            .map(|m| {
                if m.blocks.is_empty() {
                    text_to_v2(&m.role, &m.text)
                } else {
                    PortableMessageV2 {
                        version: EXPORT_VERSION_V2,
                        role: m.role.clone(),
                        blocks: m.blocks.clone(),
                    }
                }
            })
            .collect(),
    }
}

pub fn v2_to_messages(export: &PortableExportV2) -> Vec<Message> {
    export
        .messages
        .iter()
        .map(|m| {
            let content = m
                .blocks
                .iter()
                .map(|b| match b {
                    PortableBlock::Text { text } => ContentBlock::Text { text: text.clone() },
                    PortableBlock::ToolUse {
                        id,
                        name,
                        input_summary,
                    } => ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: serde_json::Value::String(input_summary.clone()),
                        parent_tool_use_id: None,
                    },
                    PortableBlock::ToolResult {
                        id,
                        output_summary,
                    } => ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: output_summary.clone(),
                    },
                    PortableBlock::Thinking { text } => ContentBlock::Thinking {
                        thinking: text.clone(),
                        // The portable format does not carry the Anthropic thinking
                        // signature; a restored block is treated as a prior-turn block
                        // (which may be sent without a signature).
                        signature: None,
                    },
                })
                .collect();
            Message {
                role: m.role.clone(),
                content,
            }
        })
        .collect()
}

pub fn v1_to_messages(messages: &[PortableMessage]) -> Vec<Message> {
    messages
        .iter()
        .map(|m| Message {
            role: m.role.clone(),
            content: vec![ContentBlock::Text { text: m.text.clone() }],
        })
        .collect()
}

pub fn messages_to_v1(messages: &[Message]) -> Vec<PortableMessage> {
    messages
        .iter()
        .map(|m| {
            let text = m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                    ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                    // MED-18: the text-only V1 format cannot represent a tool call
                    // (no id/input/result pairing). Emitting the bare tool name as
                    // prose corrupted the message and broke API round-trips, so drop
                    // ToolUse blocks — matching the documented "V1 drops tool_use".
                    ContentBlock::ToolUse { .. } => None,
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            PortableMessage {
                role: m.role.clone(),
                text,
                blocks: vec![],
            }
        })
        .collect()
}

pub fn text_to_v2(role: &str, text: &str) -> PortableMessageV2 {
    PortableMessageV2 {
        version: EXPORT_VERSION_V2,
        role: role.to_string(),
        blocks: vec![PortableBlock::Text {
            text: text.to_string(),
        }],
    }
}

pub fn v2_message_to_text(m: &PortableMessageV2) -> PortableMessage {
    let mut parts = Vec::new();
    for block in &m.blocks {
        match block {
            PortableBlock::Text { text } => parts.push(text.clone()),
            PortableBlock::ToolUse {
                name,
                input_summary,
                ..
            } => parts.push(format!("[tool:{name}] {input_summary}")),
            PortableBlock::ToolResult {
                output_summary, ..
            } => parts.push(output_summary.clone()),
            PortableBlock::Thinking { text } => parts.push(text.clone()),
        }
    }
    PortableMessage {
        role: m.role.clone(),
        text: parts.join("\n"),
        blocks: vec![],
    }
}

/// Parse a session-export JSON payload into a `ParsedExport`.
///
/// Valid inputs:
/// - A JSON array → V1 (`Vec<PortableMessage>`)
/// - A JSON object with `"version": 2` → V2 (`PortableExportV2`)
///
/// Every other shape (null, number, boolean, plain object without `version`,
/// a lone `{role, text}` object, etc.) is rejected with a descriptive error.
/// An empty array (`[]`) is valid and yields `ParsedExport::V1(vec![])`.
pub fn parse_export_json(json: &str) -> Result<ParsedExport, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    match &value {
        serde_json::Value::Array(_) => {
            // V1: array of `{role, text}` objects (may be empty).
            serde_json::from_value(value).map(ParsedExport::V1)
        }
        serde_json::Value::Object(map)
            if map.get("version").and_then(|v| v.as_u64()) == Some(EXPORT_VERSION_V2 as u64) =>
        {
            // V2: versioned export object.
            serde_json::from_value(value).map(ParsedExport::V2)
        }
        serde_json::Value::Object(_) => {
            // An object without the expected `version` field is malformed — this
            // includes lone `{role, text}` objects that look like single messages.
            Err(serde::de::Error::custom(
                "invalid export format: expected a JSON array (V1) or a versioned object (V2), \
                 got a plain object",
            ))
        }
        _ => {
            // null, boolean, number, string
            Err(serde::de::Error::custom(
                "invalid export format: expected a JSON array (V1) or a versioned object (V2)",
            ))
        }
    }
}

pub fn export_json_from_wire(messages: &[Message]) -> Result<String, serde_json::Error> {
    if messages_need_v2(messages) {
        serde_json::to_string(&messages_to_export_v2(messages))
    } else {
        serde_json::to_string(&messages_to_v1(messages))
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
    fn v2_export_includes_tool_blocks() {
        let msgs = vec![Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
                parent_tool_use_id: None,
            }],
        }];
        let json = export_json_from_wire(&msgs).unwrap();
        let parsed = parse_export_json(&json).unwrap();
        match parsed {
            ParsedExport::V2(exp) => {
                assert_eq!(exp.version, 2);
                assert!(matches!(
                    exp.messages[0].blocks[0],
                    PortableBlock::ToolUse { .. }
                ));
            }
            ParsedExport::V1(_) => panic!("expected v2 export"),
        }
    }

    #[test]
    fn messages_to_v1_drops_tool_use_keeps_text() {
        // MED-18: a tool_use block must not leak its name into the V1 text; text
        // and tool_result content are preserved, tool_use is dropped.
        let msgs = vec![Message {
            role: "assistant".into(),
            content: vec![
                ContentBlock::Text { text: "before".into() },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                    parent_tool_use_id: None,
                },
                ContentBlock::Text { text: "after".into() },
            ],
        }];
        let v1 = messages_to_v1(&msgs);
        assert_eq!(v1.len(), 1);
        assert_eq!(v1[0].text, "before\nafter");
        assert!(!v1[0].text.contains("bash"), "tool name must not leak: {}", v1[0].text);
    }

    #[test]
    fn v1_array_still_parses() {
        let json = r#"[{"role":"user","text":"question"},{"role":"assistant","text":"answer"}]"#;
        let parsed = parse_export_json(json).unwrap();
        match parsed {
            ParsedExport::V1(msgs) => assert_eq!(msgs.len(), 2),
            ParsedExport::V2(_) => panic!("expected v1"),
        }
    }

    #[test]
    fn v2_wrapper_parses() {
        let export = PortableExportV2 {
            version: 2,
            messages: vec![text_to_v2("user", "hi")],
        };
        let json = serde_json::to_string(&export).unwrap();
        let parsed = parse_export_json(&json).unwrap();
        assert!(matches!(parsed, ParsedExport::V2(_)));
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
            PortableBlock::ToolUse {
                id: "c1".into(),
                name: "read_file".into(),
                input_summary: serde_json::json!({"path": "/foo"}).to_string(),
            },
            PortableBlock::ToolResult { id: "c1".into(), output_summary: "file contents".into() },
        ];
        let msg = PortableMessage { role: "assistant".into(), text: "hello".into(), blocks };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: PortableMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.blocks.len(), 3);
        match &decoded.blocks[1] {
            PortableBlock::ToolUse { id, name, input_summary } => {
                assert_eq!(id, "c1");
                assert_eq!(name, "read_file");
                assert!(input_summary.contains("/foo"));
            }
            _ => panic!("expected ToolUse"),
        }
        match &decoded.blocks[2] {
            PortableBlock::ToolResult { id, output_summary } => {
                assert_eq!(id, "c1");
                assert_eq!(output_summary, "file contents");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn codex_history_exports_as_v2_preserving_tool_blocks() {
        // Bug 2: codex history carries rich `blocks` (tool turns) plus text-only turns.
        // It must export as V2 so importers reconstruct tool blocks instead of
        // flattening to empty text via the V1 path.
        let history = vec![
            PortableMessage {
                role: "assistant".into(),
                text: "[tool call]".into(),
                blocks: vec![PortableBlock::ToolUse {
                    id: "t1".into(),
                    name: "read_file".into(),
                    input_summary: "{\"path\":\"x\"}".into(),
                }],
            },
            PortableMessage {
                role: "user".into(),
                text: String::new(),
                blocks: vec![PortableBlock::ToolResult {
                    id: "t1".into(),
                    output_summary: "contents".into(),
                }],
            },
            PortableMessage::text_only("assistant", "done"),
        ];

        let export = portable_messages_to_export_v2(&history);
        assert_eq!(export.version, EXPORT_VERSION_V2);

        // Serialize → parse: must classify as V2 (object), not a V1 array.
        let json = serde_json::to_string(&export).unwrap();
        assert!(matches!(parse_export_json(&json).unwrap(), ParsedExport::V2(_)));

        // Reconstruct into wire Messages: tool blocks preserved, NO empty text block.
        let msgs = v2_to_messages(&export);
        assert_eq!(msgs.len(), 3);
        assert!(
            msgs.iter().any(|m| m
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }))),
            "ToolUse must survive the round-trip"
        );
        assert!(
            msgs.iter().any(|m| m
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }))),
            "ToolResult must survive the round-trip"
        );
        assert!(
            !msgs.iter().any(|m| m
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.is_empty()))),
            "no empty text block (would trigger an Anthropic 400)"
        );
        // The text-only turn became a real Text block.
        let last = msgs.last().unwrap();
        assert!(matches!(last.content.first(), Some(ContentBlock::Text { text }) if text == "done"));
    }
}
