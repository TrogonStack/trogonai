use async_nats::Client;
use bytes::Bytes;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    store::{MemoryClient, MemoryStore},
    types::{DreamerError, RawFact},
};

/// NATS subject on which in-session write requests are received.
pub const WRITE_SUBJECT: &str = "sessions.memory.write";

/// Request payload for an in-session memory write.
///
/// Sent as a NATS request to `sessions.memory.write`; the service replies with
/// [`WriteResponse`].
#[derive(Debug, Deserialize)]
pub struct WriteRequest {
    pub actor_type: String,
    pub actor_key: String,
    pub session_id: String,
    pub facts: Vec<RawFact>,
}

/// Reply payload from the memory writer.
#[derive(Debug, Serialize, Deserialize)]
pub struct WriteResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_facts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WriteResponse {
    fn success(total_facts: usize) -> Self {
        Self { ok: true, total_facts: Some(total_facts), error: None }
    }

    fn failure(error: impl Into<String>) -> Self {
        Self { ok: false, total_facts: None, error: Some(error.into()) }
    }
}

#[derive(Debug)]
pub enum WriterError {
    Subscribe(String),
}

impl std::fmt::Display for WriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriterError::Subscribe(e) => write!(f, "subscribe failed: {e}"),
        }
    }
}

impl std::error::Error for WriterError {}

/// Handles in-session memory writes: deserializes a [`WriteRequest`], merges
/// facts into the entity's memory, and returns a [`WriteResponse`].
///
/// Separate from [`MemoryWriter`] so it can be unit-tested without a live NATS
/// connection.
pub struct MemoryWriteHandler<S: MemoryStore> {
    client: MemoryClient<S>,
}

impl<S: MemoryStore> MemoryWriteHandler<S> {
    pub fn new(store: S) -> Self {
        Self { client: MemoryClient::new(store) }
    }

    /// Process a raw NATS payload and return the response bytes.
    pub async fn handle(&self, payload: &Bytes) -> WriteResponse {
        let request: WriteRequest = match serde_json::from_slice(payload) {
            Ok(r) => r,
            Err(e) => return WriteResponse::failure(format!("invalid request: {e}")),
        };

        if request.facts.is_empty() {
            return WriteResponse::failure("no facts provided");
        }

        match self.apply(&request).await {
            Ok(total) => WriteResponse::success(total),
            Err(e) => WriteResponse::failure(e.to_string()),
        }
    }

    async fn apply(&self, request: &WriteRequest) -> Result<usize, DreamerError> {
        let mut memory = self
            .client
            .get(&request.actor_type, &request.actor_key)
            .await?
            .unwrap_or_default();

        memory.merge(request.facts.clone(), &request.session_id);

        self.client
            .put(&request.actor_type, &request.actor_key, &memory)
            .await?;

        Ok(memory.facts.len())
    }
}

/// Listens on `sessions.memory.write` and persists facts into KV for each
/// request, replying immediately with the updated fact count.
///
/// Agents call [`write_memory`] to reach this service from within an active
/// session.
pub struct MemoryWriter<S: MemoryStore> {
    nats: Client,
    handler: MemoryWriteHandler<S>,
}

impl<S: MemoryStore> MemoryWriter<S> {
    pub fn new(nats: Client, store: S) -> Self {
        Self { nats, handler: MemoryWriteHandler::new(store) }
    }

    pub async fn run(self) -> Result<(), WriterError> {
        let mut sub = self
            .nats
            .subscribe(WRITE_SUBJECT)
            .await
            .map_err(|e| WriterError::Subscribe(e.to_string()))?;

        info!(subject = WRITE_SUBJECT, "Memory writer listening for in-session write requests");

        while let Some(msg) = sub.next().await {
            let Some(reply) = msg.reply.clone() else {
                warn!("Write request has no reply subject, skipping");
                continue;
            };

            let response = self.handler.handle(&msg.payload).await;

            let bytes = serde_json::to_vec(&response).unwrap_or_else(|_| {
                br#"{"ok":false,"error":"serialization failed"}"#.to_vec()
            });

            self.nats.publish(reply, bytes.into()).await.ok();
        }

        Ok(())
    }
}

/// Write facts to memory from within an active agent session.
///
/// Sends a NATS request-reply to the [`MemoryWriter`] service and waits for
/// confirmation. Returns the total number of facts now stored for the entity.
pub async fn write_memory(
    nats: &Client,
    actor_type: &str,
    actor_key: &str,
    session_id: &str,
    facts: Vec<RawFact>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let request = serde_json::json!({
        "actor_type": actor_type,
        "actor_key": actor_key,
        "session_id": session_id,
        "facts": facts,
    });
    let payload = serde_json::to_vec(&request)?;

    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        nats.request(WRITE_SUBJECT, payload.into()),
    )
    .await??;

    let response: WriteResponse = serde_json::from_slice(&reply.payload)?;
    if response.ok {
        Ok(response.total_facts.unwrap_or(0))
    } else {
        Err(response.error.unwrap_or_else(|| "unknown error".into()).into())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::mock::MockMemoryStore;

    fn handler() -> MemoryWriteHandler<MockMemoryStore> {
        MemoryWriteHandler::new(MockMemoryStore::new())
    }

    fn req(actor_type: &str, actor_key: &str, session_id: &str, facts: Vec<RawFact>) -> Bytes {
        serde_json::to_vec(&serde_json::json!({
            "actor_type": actor_type,
            "actor_key": actor_key,
            "session_id": session_id,
            "facts": facts,
        }))
        .unwrap()
        .into()
    }

    fn fact(category: &str, content: &str) -> RawFact {
        RawFact { category: category.into(), content: content.into(), confidence: 0.9 }
    }

    #[tokio::test]
    async fn write_creates_memory_for_new_entity() {
        let h = handler();
        let payload = req("pr", "owner/repo/1", "sess-1", vec![fact("preference", "uses Rust")]);
        let resp = h.handle(&payload).await;
        assert!(resp.ok);
        assert_eq!(resp.total_facts, Some(1));
    }

    #[tokio::test]
    async fn write_appends_to_existing_memory() {
        let store = MockMemoryStore::new();
        let h = MemoryWriteHandler::new(store.clone());

        let p1 = req("pr", "repo/1", "sess-1", vec![fact("goal", "ship by Friday")]);
        h.handle(&p1).await;

        let p2 = req("pr", "repo/1", "sess-1", vec![fact("preference", "dark mode")]);
        let resp = h.handle(&p2).await;

        assert!(resp.ok);
        assert_eq!(resp.total_facts, Some(2));
    }

    #[tokio::test]
    async fn write_returns_cumulative_total() {
        let store = MockMemoryStore::new();
        let h = MemoryWriteHandler::new(store);

        let p1 = req("pr", "repo/1", "s1", vec![fact("a", "1"), fact("b", "2")]);
        h.handle(&p1).await;
        let p2 = req("pr", "repo/1", "s1", vec![fact("c", "3")]);
        let resp = h.handle(&p2).await;

        assert_eq!(resp.total_facts, Some(3));
    }

    #[tokio::test]
    async fn different_entities_are_isolated() {
        let store = MockMemoryStore::new();
        let h = MemoryWriteHandler::new(store.clone());

        h.handle(&req("pr", "repo/1", "s1", vec![fact("a", "entity A")])).await;
        let resp = h.handle(&req("pr", "repo/2", "s1", vec![fact("b", "entity B")])).await;

        assert_eq!(resp.total_facts, Some(1), "repo/2 should only have its own fact");
    }

    #[tokio::test]
    async fn empty_facts_returns_error() {
        let h = handler();
        let payload = req("pr", "repo/1", "s1", vec![]);
        let resp = h.handle(&payload).await;
        assert!(!resp.ok);
        assert!(resp.error.as_deref().unwrap_or("").contains("no facts"));
    }

    #[tokio::test]
    async fn invalid_json_returns_error() {
        let h = handler();
        let payload = Bytes::from(b"not json".to_vec());
        let resp = h.handle(&payload).await;
        assert!(!resp.ok);
        assert!(resp.error.as_deref().unwrap_or("").contains("invalid request"));
    }

    #[tokio::test]
    async fn missing_field_returns_error() {
        let h = handler();
        let payload: Bytes =
            serde_json::to_vec(&serde_json::json!({
                "actor_type": "pr",
                "session_id": "s1",
                "facts": []
            }))
            .unwrap()
            .into();
        let resp = h.handle(&payload).await;
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn write_response_success_serializes_correctly() {
        let r = WriteResponse::success(5);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"total_facts\":5"));
        assert!(!json.contains("error"));
    }

    #[tokio::test]
    async fn write_response_failure_serializes_correctly() {
        let r = WriteResponse::failure("something went wrong");
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"error\":\"something went wrong\""));
        assert!(!json.contains("total_facts"));
    }
}
