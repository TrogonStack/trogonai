//! Context compaction for xai-runner via the `trogon-compactor` NATS service (protobuf wire).

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
const SESSION_PROVIDER: &str = "xai";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

fn to_proto_messages(history: &[Message]) -> Vec<ProtoMessage> {
    history
        .iter()
        .map(|m| ProtoMessage {
            role: m.role.clone(),
            content: vec![ProtoBlock {
                kind: Some(BlockKind::Text(Box::new(trogonai_compactor_proto::TextBlock {
                    text: m.content.clone().unwrap_or_default(),
                    __buffa_unknown_fields: Default::default(),
                }))),
                __buffa_unknown_fields: Default::default(),
            }],
            __buffa_unknown_fields: Default::default(),
        })
        .collect()
}

fn from_proto_messages(wire: Vec<ProtoMessage>) -> Vec<Message> {
    wire.into_iter()
        .map(|m| {
            let text = m
                .content
                .iter()
                .filter_map(|b| match b.kind.as_ref()? {
                    BlockKind::Text(t) => Some(t.text.as_str()),
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

fn encode_request(
    history: &[Message],
    model: &str,
    compactor_provider: Option<&str>,
    compactor_model: Option<&str>,
    context_window: u64,
) -> Vec<u8> {
    ProtoRequest {
        messages: to_proto_messages(history),
        provider: SESSION_PROVIDER.to_string(),
        model: model.to_string(),
        context_window,
        compactor_provider: compactor_provider.map(str::to_string),
        compactor_model: compactor_model.map(str::to_string),
        __buffa_unknown_fields: Default::default(),
    }
    .encode_to_vec()
}

fn decode_response(payload: &[u8]) -> Option<(Vec<Message>, bool, usize)> {
    if let Ok(err) = ProtoCompactError::decode_from_slice(payload)
        && !err.error.is_empty()
    {
        return None;
    }
    let resp = ProtoResponse::decode_from_slice(payload).ok()?;
    if resp.compacted {
        Some((from_proto_messages(resp.messages), true, resp.kept_count as usize))
    } else {
        None
    }
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
/// `(new_history, compacted)`. On any failure returns the original history
/// unchanged with `compacted == false`.
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

    match nats.send_request(COMPACT_SUBJECT, request).await {
        Ok(reply) => match decode_response(&reply.payload) {
            Some((new_history, true, _)) => (new_history, true),
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
    fn proto_round_trip_preserves_text() {
        let history = vec![msg("user", "hello"), msg("assistant", "hi there")];
        let proto = to_proto_messages(&history);
        let restored = from_proto_messages(proto);
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
    fn encode_request_includes_compactor_provider() {
        let history = vec![msg("user", "hi")];
        let bytes = encode_request(&history, "grok-4", Some("anthropic"), Some("claude-haiku"), 131_072);
        let proto = ProtoRequest::decode_from_slice(&bytes).unwrap();
        assert_eq!(proto.provider, "xai");
        assert_eq!(proto.compactor_provider.as_deref(), Some("anthropic"));
        assert_eq!(proto.compactor_model.as_deref(), Some("claude-haiku"));
    }
}
