//! Context compaction for xai-runner via the shared `trogon-compactor` helper.
//!
//! The wire protocol, NATS request-reply, decoding, and fallback logging all live
//! in [`trogon_runner_tools::request_compaction`]. This module keeps only the
//! xai-specific concerns: converting the runner's [`Message`] type to/from the
//! shared [`trogon_tools::Message`], the compaction gate ([`should_compact`]), and
//! the request timeout.

use std::time::Duration;

use async_nats::Client;
use trogon_runner_tools::{CompactProviders, request_compaction};
use trogon_tools::{ContentBlock, Message as ToolMessage};

use crate::client::Message;

const SESSION_PROVIDER: &str = "xai";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Convert the runner's text-only [`Message`] into the shared tool [`ToolMessage`].
fn to_tool_messages(history: &[Message]) -> Vec<ToolMessage> {
    history
        .iter()
        .map(|m| ToolMessage {
            role: m.role.clone(),
            content: vec![ContentBlock::Text {
                text: m.content.clone().unwrap_or_default(),
            }],
        })
        .collect()
}

/// Convert shared tool messages back into the runner's [`Message`] type, flattening
/// each message's text blocks into the single `content` string.
fn from_tool_messages(wire: Vec<ToolMessage>) -> Vec<Message> {
    wire.into_iter()
        .map(|m| {
            let text = m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
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

fn request_timeout() -> Duration {
    let secs = std::env::var("COMPACTOR_REQUEST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Sends the history to the compactor service and returns
/// `(new_history, compacted)`. On any failure (compactor down, timeout, invalid
/// response, or "not compacted") returns the original history unchanged with
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
        Ok(Some(resp)) => (from_tool_messages(resp.messages), true),
        Ok(None) | Err(_) => (history, false),
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
    fn tool_message_round_trip_preserves_text() {
        let history = vec![msg("user", "hello"), msg("assistant", "hi there")];
        let tool = to_tool_messages(&history);
        let restored = from_tool_messages(tool);
        assert_eq!(restored.len(), 2);
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
        let history = vec![msg("user", &"x".repeat(40))];
        assert!(should_compact(&history, 11));
        assert!(!should_compact(&history, 1000));
        assert!(!should_compact(&history, 0));
    }

    #[test]
    fn payload_carries_session_and_compactor_identity() {
        // The wire payload is built by the shared helper's `encode_compact_request`;
        // assert it stamps the xai session provider plus the compactor override.
        use buffa::Message as _;
        use trogon_runner_tools::encode_compact_request;
        use trogonai_compactor_proto::CompactRequest as ProtoRequest;

        let history = vec![msg("user", "hi")];
        let messages = to_tool_messages(&history);
        let bytes = encode_compact_request(
            &messages,
            SESSION_PROVIDER,
            "grok-4",
            Some(131_072),
            Some("anthropic"),
            Some("claude-haiku"),
        );
        let proto = ProtoRequest::decode_from_slice(&bytes).unwrap();
        assert_eq!(proto.provider, "xai");
        assert_eq!(proto.model, "grok-4");
        assert_eq!(proto.context_window, 131_072);
        assert_eq!(proto.compactor_provider.as_deref(), Some("anthropic"));
        assert_eq!(proto.compactor_model.as_deref(), Some("claude-haiku"));
    }
}
