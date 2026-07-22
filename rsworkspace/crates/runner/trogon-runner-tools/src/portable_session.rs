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
        Self {
            role: role.into(),
            text: text.into(),
            blocks: vec![],
        }
    }
}

pub const EXPORT_VERSION_V2: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PortableBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        /// Human-readable / back-compat summary of the input (may be truncated). Kept so
        /// N-1 readers that predate `input` still render something.
        input_summary: String,
        /// Full structured tool input (cambio-modelo.md §11 No-Lossy: "input JSON
        /// completo"; §637 "un modelo con tools recibe tool calls estructurados"). Absent
        /// (null) in legacy V2 payloads written before this field existed — readers then
        /// fall back to `input_summary`.
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        input: serde_json::Value,
        /// Parent tool-use id for nested/sub-agent tool calls (§11 No-Lossy lists
        /// `parent_tool_use_id` among fields that must not be dropped).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    ToolResult {
        id: String,
        /// Human-readable / back-compat summary (may be truncated). Kept for N-1 readers.
        output_summary: String,
        /// Full tool result content (cambio-modelo.md §11 No-Lossy: "output completo o
        /// referencia a artefacto"; §828). Absent in legacy V2 payloads → readers fall
        /// back to `output_summary`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
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
        m.content
            .iter()
            .any(|b| !matches!(b, ContentBlock::Text { .. } | ContentBlock::Image { .. }))
    })
}

pub fn message_to_v2(m: &Message) -> PortableMessageV2 {
    let blocks = m
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => PortableBlock::Text { text: text.clone() },
            ContentBlock::ToolUse {
                id,
                name,
                input,
                parent_tool_use_id,
            } => PortableBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input_summary: summarize_value(input),
                // Carry the FULL structured input + parent linkage (no-lossy); the
                // truncated `input_summary` is retained only for legacy display.
                input: input.clone(),
                parent_tool_use_id: parent_tool_use_id.clone(),
            },
            ContentBlock::ToolResult {
                tool_use_id, content, ..
            } => PortableBlock::ToolResult {
                id: tool_use_id.clone(),
                output_summary: truncate_str(content, 500),
                // Carry the FULL result content (no-lossy); `output_summary` is only a
                // truncated display preview.
                output: Some(content.clone()),
            },
            ContentBlock::Thinking { thinking, .. } => PortableBlock::Thinking { text: thinking.clone() },
            ContentBlock::Image { .. } => PortableBlock::Text { text: "[image]".into() },
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
                        input,
                        parent_tool_use_id,
                    } => ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        // Prefer the full structured input; fall back to the summary string
                        // only for legacy V2 payloads that predate the `input` field.
                        input: if input.is_null() {
                            serde_json::Value::String(input_summary.clone())
                        } else {
                            input.clone()
                        },
                        parent_tool_use_id: parent_tool_use_id.clone(),
                    },
                    PortableBlock::ToolResult {
                        id,
                        output_summary,
                        output,
                    } => ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        // Prefer the full content; fall back to the summary for legacy payloads.
                        content: output.clone().unwrap_or_else(|| output_summary.clone()),
                        blocks: vec![],
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
        blocks: vec![PortableBlock::Text { text: text.to_string() }],
    }
}

pub fn v2_message_to_text(m: &PortableMessageV2) -> PortableMessage {
    let mut parts = Vec::new();
    for block in &m.blocks {
        match block {
            PortableBlock::Text { text } => parts.push(text.clone()),
            PortableBlock::ToolUse {
                name, input_summary, ..
            } => parts.push(format!("[tool:{name}] {input_summary}")),
            PortableBlock::ToolResult { output_summary, .. } => parts.push(output_summary.clone()),
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
                assert!(matches!(exp.messages[0].blocks[0], PortableBlock::ToolUse { .. }));
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
                input: serde_json::json!({"path": "/foo"}),
                parent_tool_use_id: Some("parent_1".into()),
            },
            PortableBlock::ToolResult {
                id: "c1".into(),
                output_summary: "file contents".into(),
                output: Some("file contents".into()),
            },
        ];
        let msg = PortableMessage {
            role: "assistant".into(),
            text: "hello".into(),
            blocks,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: PortableMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.blocks.len(), 3);
        match &decoded.blocks[1] {
            PortableBlock::ToolUse {
                id,
                name,
                input_summary,
                input,
                parent_tool_use_id,
            } => {
                assert_eq!(id, "c1");
                assert_eq!(name, "read_file");
                assert!(input_summary.contains("/foo"));
                // The full structured input and parent linkage round-trip through serde.
                assert_eq!(input, &serde_json::json!({"path": "/foo"}));
                assert_eq!(parent_tool_use_id.as_deref(), Some("parent_1"));
            }
            _ => panic!("expected ToolUse"),
        }
        match &decoded.blocks[2] {
            PortableBlock::ToolResult { id, output_summary, output } => {
                assert_eq!(id, "c1");
                assert_eq!(output_summary, "file contents");
                assert_eq!(output.as_deref(), Some("file contents"));
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
                    input: serde_json::json!({"path": "x"}),
                    parent_tool_use_id: None,
                }],
            },
            PortableMessage {
                role: "user".into(),
                text: String::new(),
                blocks: vec![PortableBlock::ToolResult {
                    id: "t1".into(),
                    output_summary: "contents".into(),
                    output: Some("contents".into()),
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
            msgs.iter()
                .any(|m| m.content.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }))),
            "ToolUse must survive the round-trip"
        );
        assert!(
            msgs.iter()
                .any(|m| m.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }))),
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

    #[test]
    fn truncate_str_does_not_panic_on_multibyte_boundary() {
        let s = "日本語".repeat(100);
        let truncated = truncate_str(&s, 50);
        assert!(truncated.ends_with('…'));
        assert!(truncated.len() <= 50);
    }

    // cambio-modelo.md §11 (No-Lossy: "input JSON completo ... parent_tool_use_id") and
    // §637 ("un modelo con tools recibe tool calls estructurados"): a tool call's full
    // structured input and parent linkage must survive the V2 export → import round trip,
    // not be flattened to a truncated string (the old lossy behavior).
    #[test]
    fn v2_round_trip_preserves_structured_tool_input_and_parent() {
        // A large input (> the 240-char summary cap) that is also a structured object.
        let big_path = "/".to_string() + &"a".repeat(300);
        let input = serde_json::json!({ "path": big_path, "recursive": true, "depth": 5 });
        let original = vec![Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "read_file".into(),
                input: input.clone(),
                parent_tool_use_id: Some("toolu_parent".into()),
            }],
        }];

        let exported = messages_to_export_v2(&original);
        // Serialize + deserialize so we also exercise the serde wire format.
        let json = serde_json::to_string(&exported).unwrap();
        let decoded: PortableExportV2 = serde_json::from_str(&json).unwrap();
        let round_tripped = v2_to_messages(&decoded);

        match &round_tripped[0].content[0] {
            ContentBlock::ToolUse {
                id,
                name,
                input: got_input,
                parent_tool_use_id,
            } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "read_file");
                // Structured object preserved exactly — NOT collapsed to a string and NOT
                // truncated, even though it exceeds the summary cap.
                assert_eq!(got_input, &input, "full structured input must survive the round trip");
                assert!(got_input.is_object(), "input must remain a JSON object, not a string");
                assert_eq!(parent_tool_use_id.as_deref(), Some("toolu_parent"), "parent linkage preserved");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    // cambio-modelo.md §11 (No-Lossy: tool "output completo o referencia a artefacto") and
    // §828: a tool result's full content must survive the V2 round trip, not be silently
    // truncated (the old behavior truncated to 500 chars without a marker or artifact ref).
    #[test]
    fn v2_round_trip_preserves_full_tool_result_content() {
        let long_output = "line\n".repeat(400); // ~2000 chars, well over the 500 summary cap
        let original = vec![Message {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: long_output.clone(),
                blocks: vec![],
            }],
        }];

        let exported = messages_to_export_v2(&original);
        let json = serde_json::to_string(&exported).unwrap();
        let decoded: PortableExportV2 = serde_json::from_str(&json).unwrap();
        let round_tripped = v2_to_messages(&decoded);

        match &round_tripped[0].content[0] {
            ContentBlock::ToolResult { tool_use_id, content, .. } => {
                assert_eq!(tool_use_id, "toolu_1");
                assert_eq!(content, &long_output, "full tool result content must survive untruncated");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    // N-1 compatibility: a legacy V2 payload written before the `input`/`parent_tool_use_id`
    // fields existed (only `input_summary`) still imports — falling back to the summary
    // string (cambio-modelo.md §1010 Schema Versioning: readers accept the prior version).
    #[test]
    fn v2_legacy_tooluse_without_input_field_falls_back_to_summary() {
        let json = r#"{"version":2,"messages":[{"version":2,"role":"assistant","blocks":[{"type":"tool_use","id":"t1","name":"bash","input_summary":"{\"cmd\":\"ls\"}"}]}]}"#;
        let decoded: PortableExportV2 = serde_json::from_str(json).unwrap();
        let messages = v2_to_messages(&decoded);
        match &messages[0].content[0] {
            ContentBlock::ToolUse {
                input,
                parent_tool_use_id,
                ..
            } => {
                assert_eq!(input, &serde_json::Value::String("{\"cmd\":\"ls\"}".into()));
                assert!(parent_tool_use_id.is_none());
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }
}
