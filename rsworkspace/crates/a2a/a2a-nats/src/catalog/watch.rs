//! Catalog KV watch event decoding.
//!
//! The live JetStream KV watch wrapper lands in the integration PR alongside
//! its smoke harness so it can be exercised end-to-end; this slice ships the
//! pure decoder (`map_kv_entry`) and the event/error shapes downstream code
//! pattern-matches on.

use async_nats::jetstream::kv::{self, Operation};

use crate::agent_id::A2aAgentId;

#[derive(Debug, Clone, PartialEq)]
pub enum AgentCardWatchEvent {
    Put {
        agent_id: A2aAgentId,
        card: Box<a2a::agent_card::AgentCard>,
        revision: u64,
    },
    Delete {
        agent_id: A2aAgentId,
        revision: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AgentCardWatchError {
    #[error("KV watch error: {0}")]
    Kv(String),
    #[error("invalid catalog key: {0}")]
    InvalidKey(String),
    #[error("failed to deserialize AgentCard: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("AgentCard schema validation failed: {0}")]
    Schema(#[source] a2a_pack::AgentCardValidateError),
}

pub fn map_kv_entry(entry: kv::Entry) -> Result<AgentCardWatchEvent, AgentCardWatchError> {
    let agent_id = A2aAgentId::new(entry.key.as_str()).map_err(|_| {
        AgentCardWatchError::InvalidKey(format!("catalog KV key `{}` is not a valid agent id", entry.key))
    })?;
    let revision = entry.revision;

    match entry.operation {
        Operation::Put => {
            let parsed: serde_json::Value =
                serde_json::from_slice(&entry.value).map_err(AgentCardWatchError::Deserialize)?;
            a2a_pack::validate_agent_card_value(&parsed).map_err(AgentCardWatchError::Schema)?;
            let card: a2a::agent_card::AgentCard =
                serde_json::from_value(parsed).map_err(AgentCardWatchError::Deserialize)?;
            Ok(AgentCardWatchEvent::Put {
                agent_id,
                card: Box::new(card),
                revision,
            })
        }
        Operation::Delete | Operation::Purge => Ok(AgentCardWatchEvent::Delete { agent_id, revision }),
    }
}

#[cfg(test)]
mod tests;
