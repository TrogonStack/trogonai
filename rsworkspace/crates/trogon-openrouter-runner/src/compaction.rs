//! Context compaction for openrouter-runner via the shared `trogon-compactor` helper.
//!
//! The wire protocol, NATS request-reply, decoding, and fallback logging all live
//! in [`trogon_runner_tools::request_compaction`]. This module keeps only the
//! openrouter-specific concerns: converting the runner's [`Message`] type (with its
//! OpenAI-style `tool_calls`/`tool_call_id`) to/from the shared
//! [`trogon_tools::Message`], the compaction gate ([`should_compact`]), the request
//! timeout, and rebuilding the post-compaction history (summary pair + original
//! tail).

use std::time::Duration;

use async_nats::Client;
use trogon_runner_tools::{CompactProviders, CompactWireResponse, request_compaction};
use trogon_tools::{ContentBlock, Message as ToolMessage};

use crate::client::Message;

const SESSION_PROVIDER: &str = "openrouter";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

fn to_tool_messages(history: &[Message]) -> Vec<ToolMessage> {
    history.iter().map(message_to_tool).collect()
}

fn message_to_tool(m: &Message) -> ToolMessage {
    if m.role == "tool" {
        ToolMessage {
            role: "user".into(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: m.tool_call_id.clone().unwrap_or_default(),
                content: m.content.clone(),
            }],
        }
    } else if m.role == "assistant" && m.tool_calls.is_some() {
        let mut content = Vec::new();
        if !m.content.is_empty() {
            content.push(ContentBlock::Text {
                text: m.content.clone(),
            });
        }
        for tc in m.tool_calls.as_deref().unwrap_or(&[]) {
            content.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null),
                parent_tool_use_id: None,
            });
        }
        ToolMessage {
            role: "assistant".into(),
            content,
        }
    } else {
        ToolMessage {
            role: m.role.clone(),
            content: vec![ContentBlock::Text {
                text: m.content.clone(),
            }],
        }
    }
}

/// Flatten a shared tool message's text blocks into a single string.
fn tool_text(m: &ToolMessage) -> String {
    m.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
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

/// Rebuild the post-compaction history: keep the compactor's summary pair (first
/// two messages) and re-attach the original tail by `kept_count`. Bails to the
/// original history (`compacted == false`) when the response is malformed.
fn rebuild(history: Vec<Message>, resp: CompactWireResponse) -> (Vec<Message>, bool) {
    if resp.messages.len() < 2 || resp.kept_count > history.len() {
        return (history, false);
    }

    let mut new_history: Vec<Message> = resp.messages[..2]
        .iter()
        .map(|m| Message {
            role: m.role.clone(),
            content: tool_text(m),
            prompt_tokens: None,
            completion_tokens: None,
            tool_calls: None,
            tool_call_id: None,
        })
        .collect();

    let tail_start = history.len() - resp.kept_count;
    new_history.extend_from_slice(&history[tail_start..]);
    (new_history, true)
}

/// Sends the history to the compactor service and returns `(new_history,
/// compacted)`. On any failure (compactor down, timeout, invalid response, or
/// "not compacted") returns the original history unchanged with
/// `compacted == false`.
pub async fn compact(
    nats: &Client,
    history: Vec<Message>,
    model: &str,
    compactor_provider: Option<&str>,
    compactor_model: Option<&str>,
    context_window: u64,
) -> (Vec<Message>, bool) {
    let messages = to_tool_messages(&history);
    let providers = CompactProviders {
        session_provider: SESSION_PROVIDER,
        session_model: model,
        compactor_provider,
        compactor_model,
    };

    match request_compaction(nats, &messages, Some(context_window), providers, request_timeout()).await {
        Ok(Some(resp)) => rebuild(history, resp),
        Ok(None) | Err(_) => (history, false),
    }
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
        let tool_msgs = to_tool_messages(&history);
        assert_eq!(tool_msgs.len(), history.len());
    }

    fn resp(summary: &str, kept_count: usize, compacted: bool) -> CompactWireResponse {
        CompactWireResponse {
            messages: vec![
                ToolMessage {
                    role: "user".into(),
                    content: vec![ContentBlock::Text { text: summary.into() }],
                },
                ToolMessage {
                    role: "assistant".into(),
                    content: vec![ContentBlock::Text { text: "ack".into() }],
                },
            ],
            compacted,
            tokens_before: 0,
            tokens_after: 0,
            kept_count,
            fallback_model: None,
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
    fn rebuild_bails_on_short_response() {
        let history = vec![user("a"), user("b")];
        let mut r = resp("S", 1, true);
        r.messages.truncate(1);
        let (out, compacted) = rebuild(history, r);
        assert!(!compacted);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn rebuild_bails_when_kept_count_exceeds_history() {
        let history = vec![user("a")];
        let (out, compacted) = rebuild(history, resp("S", 99, true));
        assert!(!compacted);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn should_compact_unknown_window_is_false() {
        assert!(!should_compact(&[user(&"x".repeat(400))], None));
    }

    #[test]
    fn payload_carries_session_and_compactor_identity() {
        // The wire payload is built by the shared helper's `encode_compact_request`;
        // assert it stamps the openrouter session provider plus the compactor override.
        use buffa::Message as _;
        use trogon_runner_tools::encode_compact_request;
        use trogonai_compactor_proto::CompactRequest as ProtoRequest;

        let history = vec![user("hi")];
        let messages = to_tool_messages(&history);
        let bytes = encode_compact_request(
            &messages,
            SESSION_PROVIDER,
            "anthropic/claude-3.5-sonnet",
            Some(128_000),
            Some("anthropic"),
            Some("claude-haiku"),
        );
        let proto = ProtoRequest::decode_from_slice(&bytes).unwrap();
        assert_eq!(proto.provider, "openrouter");
        assert_eq!(proto.context_window, 128_000);
        assert_eq!(proto.compactor_provider.as_deref(), Some("anthropic"));
        assert_eq!(proto.compactor_model.as_deref(), Some("claude-haiku"));
    }
}
