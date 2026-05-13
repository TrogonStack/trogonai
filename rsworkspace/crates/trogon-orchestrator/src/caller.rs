use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use trogon_registry::AgentCapability;

use crate::types::OrchestratorError;

/// Abstraction over the mechanism used to invoke a sub-agent.
///
/// In production this sends a NATS request-reply. In tests a mock returns
/// preconfigured responses.
pub trait AgentCaller: Send + Sync + Clone + 'static {
    fn call<'a>(
        &'a self,
        capability: &'a AgentCapability,
        payload: Bytes,
    ) -> impl Future<Output = Result<Bytes, OrchestratorError>> + Send + 'a;
}

// ── NATS implementation ───────────────────────────────────────────────────────

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Sends a NATS request to the agent's advertised `nats_subject` and waits for
/// one reply. Uses the first concrete subject (replaces `>` with the payload
/// routing suffix so the agent inbox receives it).
#[derive(Clone)]
pub struct NatsAgentCaller {
    nats: async_nats::Client,
    timeout: Duration,
}

impl NatsAgentCaller {
    pub fn new(nats: async_nats::Client) -> Self {
        Self { nats, timeout: DEFAULT_TIMEOUT }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl AgentCaller for NatsAgentCaller {
    async fn call(
        &self,
        capability: &AgentCapability,
        payload: Bytes,
    ) -> Result<Bytes, OrchestratorError> {
        // Replace wildcard suffix with concrete "dispatch" subject.
        let subject = capability.nats_subject.trim_end_matches('>').trim_end_matches('.');
        let subject = format!("{subject}.dispatch");

        let reply = tokio::time::timeout(
            self.timeout,
            self.nats.request(subject, payload),
        )
        .await
        .map_err(|_| OrchestratorError::NoAgent(format!(
            "timeout calling agent '{}' for capability '{}'",
            capability.agent_type,
            capability.capabilities.join(",")
        )))?
        .map_err(|e| OrchestratorError::NoAgent(e.to_string()))?;

        Ok(reply.payload)
    }
}

// ── Mock implementation ───────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Returns the same bytes for every call (or an error if `error` is set).
    #[derive(Clone)]
    pub struct MockAgentCaller {
        response: Arc<Mutex<Result<Vec<u8>, String>>>,
    }

    impl MockAgentCaller {
        pub fn returning(response: impl Into<Vec<u8>>) -> Self {
            Self { response: Arc::new(Mutex::new(Ok(response.into()))) }
        }

        pub fn failing(error: impl Into<String>) -> Self {
            Self { response: Arc::new(Mutex::new(Err(error.into()))) }
        }
    }

    impl AgentCaller for MockAgentCaller {
        async fn call(
            &self,
            _capability: &AgentCapability,
            _payload: Bytes,
        ) -> Result<Bytes, OrchestratorError> {
            self.response
                .lock()
                .unwrap()
                .clone()
                .map(Bytes::from)
                .map_err(OrchestratorError::NoAgent)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::mock::MockAgentCaller;
    use super::AgentCaller;
    use bytes::Bytes;
    use trogon_registry::AgentCapability;

    fn cap() -> AgentCapability {
        AgentCapability::new("TestActor", ["review"], "actors.test.>")
    }

    #[tokio::test]
    async fn returning_gives_configured_bytes() {
        let caller = MockAgentCaller::returning(b"agent reply".to_vec());
        let result = caller.call(&cap(), Bytes::from("payload")).await.unwrap();
        assert_eq!(result.as_ref(), b"agent reply");
    }

    #[tokio::test]
    async fn failing_returns_error() {
        let caller = MockAgentCaller::failing("connection refused");
        let err = caller.call(&cap(), Bytes::new()).await.unwrap_err();
        assert!(err.to_string().contains("connection refused"));
    }

    #[tokio::test]
    async fn returning_empty_bytes_is_valid() {
        let caller = MockAgentCaller::returning(vec![]);
        let result = caller.call(&cap(), Bytes::new()).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn caller_is_callable_multiple_times() {
        let caller = MockAgentCaller::returning(b"ok".to_vec());
        for _ in 0..3 {
            let result = caller.call(&cap(), Bytes::new()).await.unwrap();
            assert_eq!(result.as_ref(), b"ok");
        }
    }
}
