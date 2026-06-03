//! Context compaction for xai-runner via the `trogon-compactor` NATS service.
//!
//! xAI history is plain text (no client-side tool calls), so conversion to the
//! compactor's Anthropic-shaped wire format is trivial: each message becomes a
//! single `Text` content block. To avoid a dependency on `trogon-compactor`,
//! the wire types are mirrored locally (the JSON is identical — Decision B).
//!
//! Compaction runs post-turn (xAI is stateful via `previous_response_id`); the
//! caller must clear `last_response_id` when this returns `compacted == true`
//! so the next turn re-sends the full compacted history.

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
    Text { text: String },
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
}

// ── Conversion (text-only, 1:1) ─────────────────────────────────────────────────

fn to_wire(history: &[Message]) -> Vec<WireMessage> {
    history
        .iter()
        .map(|m| WireMessage {
            role: m.role.clone(),
            content: vec![WireBlock::Text {
                text: m.content.clone().unwrap_or_default(),
            }],
        })
        .collect()
}

fn from_wire(wire: Vec<WireMessage>) -> Vec<Message> {
    wire.into_iter()
        .map(|m| {
            let text = m
                .content
                .into_iter()
                .map(|b| match b {
                    WireBlock::Text { text } => text,
                })
                .collect::<Vec<_>>()
                .join("");
            Message {
                role: m.role,
                content: Some(text),
                prompt_tokens: None,
                completion_tokens: None,
            }
        })
        .collect()
}

// ── Token estimation + trigger (Gap 3) ──────────────────────────────────────────

/// Rough token estimate: 1 token ≈ 4 bytes of text content.
pub fn estimate_tokens(history: &[Message]) -> u64 {
    let bytes: usize = history
        .iter()
        .map(|m| m.content.as_deref().map(str::len).unwrap_or(0))
        .sum();
    (bytes / 4) as u64
}

/// Pre-check: only call the compactor service when over 85 % of the window.
/// Avoids a NATS round-trip every turn. `context_window == 0` disables it.
pub fn should_compact(history: &[Message], context_window: u64) -> bool {
    context_window > 0 && estimate_tokens(history) > context_window * 85 / 100
}

/// Per-request NATS timeout for the compaction call. Configurable via
/// `COMPACTOR_REQUEST_TIMEOUT_SECS` (default 60 s). A summary LLM call can take
/// many seconds, so this must be generous — but always bounded (never `None`,
/// which would disable the timeout and risk a deadlock).
fn request_timeout() -> Duration {
    let secs = std::env::var("COMPACTOR_REQUEST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

// ── Compaction call ──────────────────────────────────────────────────────────────

/// Sends the history to the compactor service and returns
/// `(new_history, compacted)`. On any failure (service down, timeout, bad
/// response) returns the original history unchanged with `compacted == false` —
/// compaction never blocks or breaks a turn.
pub async fn compact(
    nats: &Client,
    history: Vec<Message>,
    model: &str,
    compactor_model: Option<&str>,
    context_window: u64,
) -> (Vec<Message>, bool) {
    let req = CompactReq {
        messages: to_wire(&history),
        provider: "xai",
        model,
        compactor_model,
        context_window,
    };
    let Ok(payload) = serde_json::to_vec(&req) else {
        return (history, false);
    };

    // Per-request timeout — never `None` (that disables the timeout → deadlock risk).
    let request = async_nats::Request::new()
        .payload(payload.into())
        .timeout(Some(request_timeout()));

    match nats.send_request(COMPACT_SUBJECT, request).await {
        Ok(reply) => match serde_json::from_slice::<CompactResp>(&reply.payload) {
            Ok(resp) if resp.compacted => (from_wire(resp.messages), true),
            _ => (history, false),
        },
        Err(_) => (history, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            content: Some(text.into()),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    #[test]
    fn to_wire_from_wire_round_trips() {
        let history = vec![msg("user", "hello"), msg("assistant", "hi there")];
        let restored = from_wire(serde_json::from_slice(&serde_json::to_vec(&to_wire(&history)).unwrap()).unwrap());
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].role, "user");
        assert_eq!(restored[0].content.as_deref(), Some("hello"));
        assert_eq!(restored[1].content.as_deref(), Some("hi there"));
    }

    #[test]
    fn estimate_tokens_is_bytes_over_four() {
        let history = vec![msg("user", &"x".repeat(40))];
        assert_eq!(estimate_tokens(&history), 10);
    }

    #[test]
    fn should_compact_respects_threshold() {
        // 40 bytes → 10 tokens. Window 11 → 85% = 9 → over.
        let history = vec![msg("user", &"x".repeat(40))];
        assert!(should_compact(&history, 11));
        // Window 1000 → 85% = 850 → under.
        assert!(!should_compact(&history, 1000));
        // Unknown window (0) → never compact.
        assert!(!should_compact(&history, 0));
    }

    #[test]
    fn request_timeout_defaults_to_60s_and_reads_env_override() {
        // Combined into one test to avoid env-var races with parallel tests.
        unsafe { std::env::remove_var("COMPACTOR_REQUEST_TIMEOUT_SECS") };
        assert_eq!(request_timeout(), Duration::from_secs(60), "default must be 60s");

        unsafe { std::env::set_var("COMPACTOR_REQUEST_TIMEOUT_SECS", "120") };
        assert_eq!(request_timeout(), Duration::from_secs(120), "env override must apply");

        // Garbage value falls back to the default (never panics, never unbounded).
        unsafe { std::env::set_var("COMPACTOR_REQUEST_TIMEOUT_SECS", "not-a-number") };
        assert_eq!(request_timeout(), Duration::from_secs(60), "bad value → default");

        unsafe { std::env::remove_var("COMPACTOR_REQUEST_TIMEOUT_SECS") };
    }

    #[test]
    fn wire_block_serializes_with_text_tag() {
        let w = to_wire(&[msg("user", "hi")]);
        let json = serde_json::to_value(&w).unwrap();
        assert_eq!(json[0]["role"], "user");
        assert_eq!(json[0]["content"][0]["type"], "text");
        assert_eq!(json[0]["content"][0]["text"], "hi");
    }
}
