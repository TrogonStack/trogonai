//! Context compaction for openrouter-runner via the `trogon-compactor` NATS service (protobuf wire).

use std::time::Duration;

use async_nats::Client;
use buffa::Message as _;
use trogonai_compactor_proto::{
    __buffa::oneof::content_block::Kind as BlockKind, CompactError as ProtoCompactError,
    CompactRequest as ProtoRequest, CompactResponse as ProtoResponse, ContentBlock as ProtoBlock,
    Message as ProtoMessage,
};

use crate::client::Message;

const COMPACT_SUBJECT: &str = "trogon.compactor.compact";
const SESSION_PROVIDER: &str = "openrouter";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

enum WireBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

struct WireMessage {
    role: String,
    content: Vec<WireBlock>,
}

fn to_wire(history: &[Message]) -> Vec<WireMessage> {
    history.iter().map(message_to_wire).collect()
}

fn message_to_wire(m: &Message) -> WireMessage {
    if m.role == "tool" {
        WireMessage {
            role: "user".into(),
            content: vec![WireBlock::ToolResult {
                tool_use_id: m.tool_call_id.clone().unwrap_or_default(),
                content: m.content.clone(),
            }],
        }
    } else if m.role == "assistant" && m.tool_calls.is_some() {
        let mut content = Vec::new();
        if !m.content.is_empty() {
            content.push(WireBlock::Text {
                text: m.content.clone(),
            });
        }
        for tc in m.tool_calls.as_deref().unwrap_or(&[]) {
            content.push(WireBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null),
            });
        }
        WireMessage {
            role: "assistant".into(),
            content,
        }
    } else {
        WireMessage {
            role: m.role.clone(),
            content: vec![WireBlock::Text {
                text: m.content.clone(),
            }],
        }
    }
}

fn wire_to_proto(wire: &[WireMessage]) -> Vec<ProtoMessage> {
    wire.iter()
        .map(|m| ProtoMessage {
            role: m.role.clone(),
            content: m.content.iter().map(block_to_proto).collect(),
            __buffa_unknown_fields: Default::default(),
        })
        .collect()
}

fn block_to_proto(b: &WireBlock) -> ProtoBlock {
    let kind = match b {
        WireBlock::Text { text } => BlockKind::Text(Box::new(trogonai_compactor_proto::TextBlock {
            text: text.clone(),
            __buffa_unknown_fields: Default::default(),
        })),
        WireBlock::ToolUse { id, name, input } => {
            BlockKind::ToolUse(Box::new(trogonai_compactor_proto::ToolUseBlock {
                id: id.clone(),
                name: name.clone(),
                input_json: input.to_string(),
                parent_tool_use_id: None,
                __buffa_unknown_fields: Default::default(),
            }))
        }
        WireBlock::ToolResult { tool_use_id, content } => {
            BlockKind::ToolResult(Box::new(trogonai_compactor_proto::ToolResultBlock {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
                __buffa_unknown_fields: Default::default(),
            }))
        }
    };
    ProtoBlock {
        kind: Some(kind),
        __buffa_unknown_fields: Default::default(),
    }
}

fn wire_text(w: &ProtoMessage) -> String {
    w.content
        .iter()
        .filter_map(|b| match b.kind.as_ref()? {
            BlockKind::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn encode_request(
    history: &[Message],
    model: &str,
    compactor_provider: Option<&str>,
    compactor_model: Option<&str>,
    context_window: u64,
) -> Vec<u8> {
    let wire = to_wire(history);
    ProtoRequest {
        messages: wire_to_proto(&wire),
        provider: SESSION_PROVIDER.to_string(),
        model: model.to_string(),
        context_window,
        compactor_provider: compactor_provider.map(str::to_string),
        compactor_model: compactor_model.map(str::to_string),
        __buffa_unknown_fields: Default::default(),
    }
    .encode_to_vec()
}

fn decode_response(payload: &[u8]) -> Option<ProtoResponse> {
    if let Ok(err) = ProtoCompactError::decode_from_slice(payload)
        && !err.error.is_empty()
    {
        return None;
    }
    ProtoResponse::decode_from_slice(payload).ok()
}

/// Rough token estimate: 1 token ≈ 4 bytes of content + tool-call arguments.
pub fn estimate_tokens(history: &[Message]) -> u64 {
    let bytes: usize = history
        .iter()
        .map(|m| {
            m.content.len()
                + m.tool_calls
                    .as_ref()
                    .map(|tcs| tcs.iter().map(|t| t.arguments.len() + t.name.len()).sum())
                    .unwrap_or(0)
        })
        .sum();
    (bytes / 4) as u64
}

pub fn should_compact(history: &[Message], context_window: Option<u64>) -> bool {
    match context_window {
        Some(cw) if cw > 0 => estimate_tokens(history) > cw * 85 / 100,
        _ => false,
    }
}

fn request_timeout() -> Duration {
    let secs = std::env::var("COMPACTOR_REQUEST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

fn rebuild(history: Vec<Message>, resp: ProtoResponse) -> (Vec<Message>, bool) {
    if !resp.compacted || resp.messages.len() < 2 || resp.kept_count as usize > history.len() {
        return (history, false);
    }

    let mut new_history: Vec<Message> = resp.messages[..2]
        .iter()
        .map(|w| Message {
            role: w.role.clone(),
            content: wire_text(w),
            prompt_tokens: None,
            completion_tokens: None,
            tool_calls: None,
            tool_call_id: None,
        })
        .collect();

    let tail_start = history.len() - resp.kept_count as usize;
    new_history.extend_from_slice(&history[tail_start..]);
    (new_history, true)
}

pub async fn compact(
    nats: &Client,
    history: Vec<Message>,
    model: &str,
    compactor_provider: Option<&str>,
    compactor_model: Option<&str>,
    context_window: u64,
) -> (Vec<Message>, bool) {
    let payload = encode_request(&history, model, compactor_provider, compactor_model, context_window);

    let request = async_nats::Request::new()
        .payload(payload.into())
        .timeout(Some(request_timeout()));

    let reply = match nats.send_request(COMPACT_SUBJECT, request).await {
        Ok(r) => r,
        Err(_) => return (history, false),
    };
    let resp = match decode_response(&reply.payload) {
        Some(r) => r,
        None => return (history, false),
    };

    rebuild(history, resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::AssembledToolCall;

    fn user(text: &str) -> Message {
        Message::user(text)
    }
    fn assistant_tc(text: &str, calls: &[(&str, &str, &str)]) -> Message {
        let assembled: Vec<AssembledToolCall> = calls
            .iter()
            .map(|(id, name, args)| AssembledToolCall {
                id: (*id).into(),
                name: (*name).into(),
                arguments: (*args).into(),
            })
            .collect();
        let mut m = Message::assistant_tool_calls(&assembled);
        if !text.is_empty() {
            m.content = text.into();
        }
        m
    }
    fn tool(id: &str, content: &str) -> Message {
        Message::tool_result(id.into(), content)
    }

    #[test]
    fn forward_conversion_is_one_to_one() {
        let history = vec![
            user("do X"),
            assistant_tc("", &[("c1", "bash", r#"{"cmd":"ls"}"#)]),
            tool("c1", "file.txt"),
            Message::assistant("done"),
        ];
        let wire = to_wire(&history);
        assert_eq!(wire.len(), history.len());
    }

    fn resp(summary: &str, kept_count: usize, compacted: bool) -> ProtoResponse {
        ProtoResponse {
            messages: vec![
                ProtoMessage {
                    role: "user".into(),
                    content: vec![ProtoBlock {
                        kind: Some(BlockKind::Text(Box::new(trogonai_compactor_proto::TextBlock {
                            text: summary.into(),
                            __buffa_unknown_fields: Default::default(),
                        }))),
                        __buffa_unknown_fields: Default::default(),
                    }],
                    __buffa_unknown_fields: Default::default(),
                },
                ProtoMessage {
                    role: "assistant".into(),
                    content: vec![ProtoBlock {
                        kind: Some(BlockKind::Text(Box::new(trogonai_compactor_proto::TextBlock {
                            text: "ack".into(),
                            __buffa_unknown_fields: Default::default(),
                        }))),
                        __buffa_unknown_fields: Default::default(),
                    }],
                    __buffa_unknown_fields: Default::default(),
                },
            ],
            compacted,
            tokens_before: 0,
            tokens_after: 0,
            kept_count: kept_count as u32,
            fallback_model: None,
            __buffa_unknown_fields: Default::default(),
        }
    }

    #[test]
    fn rebuild_reuses_original_tail() {
        let history = vec![
            user("old"),
            assistant_tc("", &[("c1", "bash", "{}")]),
            tool("c1", "out"),
            user("recent question"),
        ];
        let (new_history, compacted) = rebuild(history, resp("<context-summary>\nS\n</context-summary>", 1, true));
        assert!(compacted);
        assert_eq!(new_history.len(), 3);
        assert_eq!(new_history[2].content, "recent question");
    }

    #[test]
    fn should_compact_unknown_window_is_false() {
        assert!(!should_compact(&[user(&"x".repeat(400))], None));
    }
}
