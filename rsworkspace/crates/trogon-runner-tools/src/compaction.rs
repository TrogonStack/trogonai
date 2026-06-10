//! NATS-backed context compaction for trogon runners.
//!
//! Runners call [`maybe_compact`] when conversation history approaches the token
//! budget. If `trogon-compactor` is unavailable the call degrades gracefully.

use std::fmt;
use std::time::Duration;

use tracing::warn;
use trogon_tools::Message;

use crate::compactor_wire::{decode_compact_response, encode_compact_request};

pub const COMPACT_SUBJECT: &str = "trogon.compactor.compact";
pub const DEFAULT_TOKEN_BUDGET: usize = 200_000;
pub const DEFAULT_COMPACT_THRESHOLD_PCT: u8 = 85;

const COMPACT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub enum CompactError {
    Request(String),
    InvalidResponse(String),
    Serialize(String),
}

impl fmt::Display for CompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(s) => write!(f, "NATS request failed: {s}"),
            Self::InvalidResponse(s) => write!(f, "invalid compactor response: {s}"),
            Self::Serialize(s) => write!(f, "serialization error: {s}"),
        }
    }
}

impl std::error::Error for CompactError {}

/// Read `TOKEN_BUDGET` and `COMPACT_THRESHOLD_PCT` from the environment.
pub fn compaction_settings_from_env() -> (usize, u8) {
    let budget = std::env::var("TOKEN_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TOKEN_BUDGET);
    let threshold = std::env::var("COMPACT_THRESHOLD_PCT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_COMPACT_THRESHOLD_PCT);
    (budget, threshold)
}

/// Heuristic token estimate: serialized JSON byte length / 4.
pub fn estimate_tokens(messages: &[Message]) -> usize {
    serde_json::to_string(messages).map(|s| s.len() / 4).unwrap_or(0)
}

/// Returns `true` when [`estimate_tokens`] exceeds `threshold_pct` % of `token_budget`.
pub fn over_threshold(messages: &[Message], token_budget: usize, threshold_pct: u8) -> bool {
    estimate_tokens(messages) * 100 >= token_budget.saturating_mul(threshold_pct as usize)
}

/// Session + compactor provider/model selection for a compaction request.
pub struct CompactProviders<'a> {
    pub session_provider: &'a str,
    pub session_model: &'a str,
    pub compactor_provider: Option<&'a str>,
    pub compactor_model: Option<&'a str>,
}

/// Request compaction from `trogon-compactor` when history is over the threshold.
///
/// Returns `Ok(None)` when under threshold or the compactor chose not to compact.
/// Returns `Ok(Some(messages))` when compaction succeeded.
pub async fn maybe_compact(
    nats: &async_nats::Client,
    messages: &[Message],
    token_budget: usize,
    threshold_pct: u8,
    providers: CompactProviders<'_>,
) -> Result<Option<Vec<Message>>, CompactError> {
    if !over_threshold(messages, token_budget, threshold_pct) {
        return Ok(None);
    }

    let payload = encode_compact_request(
        messages,
        providers.session_provider,
        providers.session_model,
        None,
        providers.compactor_provider,
        providers.compactor_model,
    );

    let reply = tokio::time::timeout(COMPACT_TIMEOUT, nats.request(COMPACT_SUBJECT, payload.into()))
        .await
        .map_err(|_| CompactError::InvalidResponse("compactor request timed out".into()))?
        .map_err(|e| CompactError::Request(e.to_string()))?;

    let resp = decode_compact_response(&reply.payload)?;

    if let Some(ref fallback) = resp.fallback_model {
        warn!(fallback_model = %fallback, "compactor used fallback model");
    }

    if resp.compacted {
        Ok(Some(resp.messages))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_tools::ContentBlock;

    fn user_msg(text: &str) -> Message {
        Message {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    #[test]
    fn over_threshold_false_for_small_history() {
        let msgs = vec![user_msg("hello")];
        assert!(!over_threshold(
            &msgs,
            DEFAULT_TOKEN_BUDGET,
            DEFAULT_COMPACT_THRESHOLD_PCT
        ));
    }

    #[test]
    fn over_threshold_true_when_estimate_exceeds_pct() {
        let big = "x".repeat(DEFAULT_TOKEN_BUDGET * 4);
        let msgs = vec![user_msg(&big)];
        assert!(over_threshold(
            &msgs,
            DEFAULT_TOKEN_BUDGET,
            DEFAULT_COMPACT_THRESHOLD_PCT
        ));
    }

    #[test]
    fn compaction_settings_from_env_defaults() {
        let (budget, pct) = compaction_settings_from_env();
        assert_eq!(budget, DEFAULT_TOKEN_BUDGET);
        assert_eq!(pct, DEFAULT_COMPACT_THRESHOLD_PCT);
    }

    #[test]
    fn estimate_tokens_is_nonzero_for_messages() {
        let msgs = vec![user_msg("hello world")];
        assert!(estimate_tokens(&msgs) > 0);
    }

    #[test]
    fn gap_c_session_provider_on_wire_from_runner_identity() {
        use buffa::Message as _;
        use trogonai_compactor_proto::CompactRequest as ProtoRequest;

        let payload = encode_compact_request(&[], "xai", "grok-4", None, Some("anthropic"), Some("claude-haiku"));
        let proto = ProtoRequest::decode_from_slice(&payload).unwrap();
        assert_eq!(proto.provider, "xai");
        assert_eq!(proto.compactor_provider.as_deref(), Some("anthropic"));
        assert_eq!(proto.compactor_model.as_deref(), Some("claude-haiku"));
    }
}
