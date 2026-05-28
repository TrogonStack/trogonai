//! Context compaction for openrouter-runner via the `trogon-compactor` NATS
//! service.
//!
//! openrouter is stateless — the full history is sent on every request, so it
//! must fit the window. Compaction runs PRE-request. The forward conversion to
//! the compactor's Anthropic-shaped wire format is strictly 1:1 (one runner
//! message → one wire message).
//!
//! **Gap 2**: the reverse conversion (tool_calls / tool-result reconstruction)
//! is *eliminated*, not just tested. The compactor preserves the kept tail
//! verbatim and reports `kept_count`; we reuse our OWN original tail messages
//! and only convert the 2 prepended summary/ack messages (plain text). The wire
//! types are mirrored locally to avoid a dependency on `trogon-compactor`.

use std::time::Duration;

use async_nats::Client;
use serde::{Deserialize, Serialize};

use crate::client::Message;

const COMPACT_SUBJECT: &str = "trogon.compactor.compact";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

// ── Local mirror of the compactor wire format ───────────────────────────────────

#[derive(Serialize, Deserialize)]
struct WireMessage {
    role: String,
    content: Vec<WireBlock>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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

#[derive(Serialize)]
struct CompactReq<'a> {
    messages: Vec<WireMessage>,
    provider: &'a str,
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    compactor_model: Option<&'a str>,
    context_window: u64,
}

#[derive(Deserialize)]
struct CompactResp {
    messages: Vec<WireMessage>,
    #[serde(default)]
    compacted: bool,
    #[serde(default)]
    kept_count: usize,
}

// ── Forward conversion (1:1) ─────────────────────────────────────────────────────

fn to_wire(history: &[Message]) -> Vec<WireMessage> {
    history.iter().map(message_to_wire).collect()
}

fn message_to_wire(m: &Message) -> WireMessage {
    if m.role == "tool" {
        // OpenAI tool result → Anthropic ToolResult under a `user` message.
        WireMessage {
            role: "user".into(),
            content: vec![WireBlock::ToolResult {
                tool_use_id: m.tool_call_id.clone().unwrap_or_default(),
                content: m.content.clone(),
            }],
        }
    } else if m.role == "assistant" && m.tool_calls.is_some() {
        // Assistant with tool_calls → one message: optional Text + N ToolUse.
        let mut content = Vec::new();
        if !m.content.is_empty() {
            content.push(WireBlock::Text { text: m.content.clone() });
        }
        for tc in m.tool_calls.as_deref().unwrap_or(&[]) {
            content.push(WireBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null),
            });
        }
        WireMessage { role: "assistant".into(), content }
    } else {
        WireMessage {
            role: m.role.clone(),
            content: vec![WireBlock::Text { text: m.content.clone() }],
        }
    }
}

/// Plain-text projection of a wire message (used only for the summary/ack pair).
fn wire_text(w: &WireMessage) -> String {
    w.content
        .iter()
        .filter_map(|b| match b {
            WireBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

// ── Token estimation + trigger (Gap 3) ──────────────────────────────────────────

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

/// Pre-check: only call the service when over 85 % of the window. `None` window
/// (unknown model) → never compact (conservative; avoids a wrong threshold).
pub fn should_compact(history: &[Message], context_window: Option<u64>) -> bool {
    match context_window {
        Some(cw) if cw > 0 => estimate_tokens(history) > cw * 85 / 100,
        _ => false,
    }
}

/// Per-request NATS timeout for the compaction call. Configurable via
/// `COMPACTOR_REQUEST_TIMEOUT_SECS` (default 60 s). Always bounded (never `None`).
fn request_timeout() -> Duration {
    let secs = std::env::var("COMPACTOR_REQUEST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

// ── Compaction call (Gap 2: reuse own tail) ──────────────────────────────────────

/// Sends the history to the compactor and returns `(new_history, compacted)`.
///
/// On `compacted`, the new history is `[summary, ack]` (converted from the
/// response — plain text) followed by the runner's OWN original tail of length
/// `kept_count` — so tool_calls / tool-result pairing is never reconstructed.
/// On any failure, returns the original history unchanged.
pub async fn compact(
    nats: &Client,
    history: Vec<Message>,
    model: &str,
    compactor_model: Option<&str>,
    context_window: u64,
) -> (Vec<Message>, bool) {
    let req = CompactReq {
        messages: to_wire(&history),
        provider: "openrouter",
        model,
        compactor_model,
        context_window,
    };
    let Ok(payload) = serde_json::to_vec(&req) else {
        return (history, false);
    };

    let request = async_nats::Request::new()
        .payload(payload.into())
        .timeout(Some(request_timeout()));

    let reply = match nats.send_request(COMPACT_SUBJECT, request).await {
        Ok(r) => r,
        Err(_) => return (history, false),
    };
    let resp: CompactResp = match serde_json::from_slice(&reply.payload) {
        Ok(r) => r,
        Err(_) => return (history, false),
    };

    rebuild(history, resp)
}

/// Gap 2 core: build the new history from the response + our own tail.
fn rebuild(history: Vec<Message>, resp: CompactResp) -> (Vec<Message>, bool) {
    // Guard the invariants: only rebuild when compacted, the prepended
    // summary+ack are present, and kept_count maps into our history.
    if !resp.compacted || resp.messages.len() < 2 || resp.kept_count > history.len() {
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

    let tail_start = history.len() - resp.kept_count;
    new_history.extend_from_slice(&history[tail_start..]);
    (new_history, true)
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

    // ── forward conversion 1:1 ────────────────────────────────────────────────

    #[test]
    fn forward_conversion_is_one_to_one() {
        let history = vec![
            user("do X"),
            assistant_tc("", &[("c1", "bash", r#"{"cmd":"ls"}"#)]),
            tool("c1", "file.txt"),
            Message::assistant("done"),
        ];
        let wire = to_wire(&history);
        assert_eq!(wire.len(), history.len(), "must be strictly 1:1");
    }

    #[test]
    fn tool_message_becomes_user_tool_result() {
        let wire = to_wire(&[tool("c1", "out")]);
        assert_eq!(wire[0].role, "user");
        assert!(matches!(wire[0].content[0], WireBlock::ToolResult { .. }));
    }

    #[test]
    fn assistant_with_two_tool_calls_is_single_message_with_two_tooluse() {
        let m = assistant_tc("thinking", &[("c1", "a", "{}"), ("c2", "b", "{}")]);
        let wire = to_wire(&[m]);
        assert_eq!(wire.len(), 1, "one assistant message");
        let tool_uses = wire[0]
            .content
            .iter()
            .filter(|b| matches!(b, WireBlock::ToolUse { .. }))
            .count();
        assert_eq!(tool_uses, 2);
        assert!(matches!(wire[0].content[0], WireBlock::Text { .. }), "text first");
    }

    // ── Gap 2: rebuild reuses own tail, no reverse conversion ─────────────────

    fn resp(summary: &str, kept_count: usize, compacted: bool) -> CompactResp {
        CompactResp {
            messages: vec![
                WireMessage { role: "user".into(), content: vec![WireBlock::Text { text: summary.into() }] },
                WireMessage { role: "assistant".into(), content: vec![WireBlock::Text { text: "ack".into() }] },
            ],
            compacted,
            kept_count,
        }
    }

    #[test]
    fn rebuild_reuses_original_tail_and_keeps_tool_pairing() {
        // History: old user, assistant+tool_calls, tool result, recent user.
        let history = vec![
            user("old"),
            assistant_tc("", &[("c1", "bash", "{}")]),
            tool("c1", "out"),
            user("recent question"),
        ];
        // Compactor kept the last 1 message (the recent user turn).
        let (new_history, compacted) = rebuild(history, resp("<context-summary>\nS\n</context-summary>", 1, true));
        assert!(compacted);
        // [summary, ack, recent user] — 3 messages.
        assert_eq!(new_history.len(), 3);
        assert_eq!(new_history[0].role, "user");
        assert!(new_history[0].content.contains("context-summary"));
        assert_eq!(new_history[1].role, "assistant");
        // The kept tail is the runner's OWN original message, intact.
        assert_eq!(new_history[2].role, "user");
        assert_eq!(new_history[2].content, "recent question");
    }

    #[test]
    fn rebuild_not_compacted_returns_history_unchanged() {
        let history = vec![user("a"), user("b")];
        let (out, compacted) = rebuild(history, resp("S", 2, false));
        assert!(!compacted);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn rebuild_rejects_kept_count_larger_than_history() {
        let history = vec![user("a")];
        let (out, compacted) = rebuild(history, resp("S", 5, true));
        assert!(!compacted, "kept_count > history.len() must fall back");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn should_compact_unknown_window_is_false() {
        assert!(!should_compact(&[user(&"x".repeat(400))], None));
    }

    #[test]
    fn should_compact_respects_threshold() {
        let history = vec![user(&"x".repeat(40))]; // ~10 tokens
        assert!(should_compact(&history, Some(11)));
        assert!(!should_compact(&history, Some(1000)));
    }

    #[test]
    fn request_timeout_defaults_to_60s_and_reads_env_override() {
        unsafe { std::env::remove_var("COMPACTOR_REQUEST_TIMEOUT_SECS") };
        assert_eq!(request_timeout(), Duration::from_secs(60), "default must be 60s");

        unsafe { std::env::set_var("COMPACTOR_REQUEST_TIMEOUT_SECS", "120") };
        assert_eq!(request_timeout(), Duration::from_secs(120), "env override must apply");

        unsafe { std::env::set_var("COMPACTOR_REQUEST_TIMEOUT_SECS", "not-a-number") };
        assert_eq!(request_timeout(), Duration::from_secs(60), "bad value → default");

        unsafe { std::env::remove_var("COMPACTOR_REQUEST_TIMEOUT_SECS") };
    }
}
