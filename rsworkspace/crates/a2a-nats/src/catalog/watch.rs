use std::pin::Pin;
use std::task::{Context, Poll};

use async_nats::jetstream::kv::{self, Operation};
use futures::Stream;
use tokio_util::sync::CancellationToken;

use crate::agent_id::A2aAgentId;
#[derive(Debug, Clone, PartialEq)]
pub enum AgentCardWatchEvent {
    Put {
        agent_id: A2aAgentId,
        card: a2a_types::AgentCard,
        revision: u64,
    },
    Delete {
        agent_id: A2aAgentId,
        revision: u64,
    },
}

#[derive(Debug)]
pub enum AgentCardWatchError {
    Kv(String),
    InvalidKey(String),
    Deserialize(serde_json::Error),
    Schema(a2a_pack::AgentCardValidateError),
}

impl std::fmt::Display for AgentCardWatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kv(msg) => write!(f, "KV watch error: {msg}"),
            Self::InvalidKey(msg) => write!(f, "invalid catalog key: {msg}"),
            Self::Deserialize(e) => write!(f, "failed to deserialize AgentCard: {e}"),
            Self::Schema(e) => write!(f, "AgentCard schema validation failed: {e}"),
        }
    }
}

impl std::error::Error for AgentCardWatchError {}

pub struct AgentCardWatchStream {
    inner: kv::Watch,
    shutdown: CancellationToken,
}

impl AgentCardWatchStream {
    pub fn new(inner: kv::Watch, shutdown: CancellationToken) -> Self {
        Self { inner, shutdown }
    }

    pub async fn subscribe_agent(
        store: &kv::Store,
        agent_id: &A2aAgentId,
        shutdown: CancellationToken,
    ) -> Result<Self, AgentCardWatchError> {
        let inner = store
            .watch(agent_id.as_str())
            .await
            .map_err(|e| AgentCardWatchError::Kv(e.to_string()))?;
        Ok(Self::new(inner, shutdown))
    }
}

impl Stream for AgentCardWatchStream {
    type Item = Result<AgentCardWatchEvent, AgentCardWatchError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.shutdown.is_cancelled() {
            return Poll::Ready(None);
        }

        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(entry))) => Poll::Ready(Some(map_kv_entry(entry))),
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(AgentCardWatchError::Kv(error.to_string())))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn map_kv_entry(entry: kv::Entry) -> Result<AgentCardWatchEvent, AgentCardWatchError> {
    let agent_id = A2aAgentId::new(entry.key.as_str()).map_err(|_| {
        AgentCardWatchError::InvalidKey(format!("catalog KV key `{}` is not a valid agent id", entry.key))
    })?;
    let revision = entry.revision;

    match entry.operation {
        Operation::Put => {
            let parsed: serde_json::Value =
                serde_json::from_slice(&entry.value).map_err(AgentCardWatchError::Deserialize)?;
            a2a_pack::validate_agent_card_value(&parsed).map_err(AgentCardWatchError::Schema)?;
            let card: a2a_types::AgentCard =
                serde_json::from_value(parsed).map_err(AgentCardWatchError::Deserialize)?;
            Ok(AgentCardWatchEvent::Put {
                agent_id,
                card,
                revision,
            })
        }
        Operation::Delete | Operation::Purge => Ok(AgentCardWatchEvent::Delete { agent_id, revision }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_error_display() {
        let err = AgentCardWatchError::Kv("down".into());
        assert!(err.to_string().contains("KV watch"));
    }
}
