use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, pull},
    kv,
};
use futures_util::StreamExt as _;
use tracing::{info, warn};
use trogon_transcript::store::TranscriptStore;

use crate::{
    dreamer::Dreamer,
    provider::MemoryProvider,
    provision::DREAMS_STREAM,
    store::MemoryStore,
    types::DreamTrigger,
};

#[derive(Debug)]
pub enum ServiceError {
    GetStream(String),
    CreateConsumer(String),
    Messages(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::GetStream(e) => write!(f, "get_stream failed: {e}"),
            ServiceError::CreateConsumer(e) => write!(f, "create_consumer failed: {e}"),
            ServiceError::Messages(e) => write!(f, "messages() failed: {e}"),
        }
    }
}

impl std::error::Error for ServiceError {}

/// Subscribes to `sessions.dream.>` and runs memory extraction for each trigger.
///
/// Generic over the LLM provider (`P`) and KV store (`S`) so unit tests can
/// inject mocks; production code uses `AnthropicMemoryProvider` and `kv::Store`.
pub struct DreamingService<P: MemoryProvider, S: MemoryStore> {
    /// Used for both the dreams consumer and transcript reads.
    js: jetstream::Context,
    consumer_name: String,
    dreamer: Dreamer<P, S>,
}

impl<P: MemoryProvider, S: MemoryStore> DreamingService<P, S> {
    pub fn new(js: jetstream::Context, consumer_name: String, dreamer: Dreamer<P, S>) -> Self {
        Self { js, consumer_name, dreamer }
    }

    pub async fn run(self) -> Result<(), ServiceError> {
        let stream = self
            .js
            .get_stream(DREAMS_STREAM)
            .await
            .map_err(|e| ServiceError::GetStream(e.to_string()))?;

        let consumer = stream
            .create_consumer(pull::Config {
                durable_name: Some(self.consumer_name.clone()),
                filter_subject: "sessions.dream.>".to_string(),
                ack_policy: AckPolicy::Explicit,
                deliver_policy: DeliverPolicy::All,
                ..Default::default()
            })
            .await
            .map_err(|e| ServiceError::CreateConsumer(e.to_string()))?;

        let mut messages = consumer
            .messages()
            .await
            .map_err(|e| ServiceError::Messages(e.to_string()))?;

        info!(consumer = %self.consumer_name, "Dreaming service listening for session completions");

        while let Some(result) = messages.next().await {
            let msg = match result {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, "Error receiving NATS dream trigger");
                    continue;
                }
            };

            let trigger: DreamTrigger = match serde_json::from_slice(&msg.payload) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "Malformed dream trigger payload, skipping");
                    msg.ack().await.ok();
                    continue;
                }
            };

            info!(
                actor_type = %trigger.actor_type,
                actor_key = %trigger.actor_key,
                session_id = %trigger.session_id,
                "Processing dream trigger"
            );

            let transcript_store = TranscriptStore::new(self.js.clone());
            let transcript = match transcript_store
                .replay(&trigger.actor_type, &trigger.actor_key, &trigger.session_id)
                .await
            {
                Ok(entries) => entries,
                Err(e) => {
                    warn!(error = %e, "Failed to read transcript, skipping");
                    msg.ack().await.ok();
                    continue;
                }
            };

            match self
                .dreamer
                .process_session(
                    &trigger.actor_type,
                    &trigger.actor_key,
                    &trigger.session_id,
                    &transcript,
                )
                .await
            {
                Ok(memory) => {
                    info!(
                        actor_type = %trigger.actor_type,
                        actor_key = %trigger.actor_key,
                        facts = memory.facts.len(),
                        "Memory updated after dreaming"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Dreaming failed for session");
                }
            }

            msg.ack().await.ok();
        }

        Ok(())
    }
}

/// Publish a dream trigger to NATS so the dreaming service picks it up.
///
/// Call this at the end of a session (e.g. from `trogon-acp-runner` or an actor)
/// to schedule memory extraction.
pub async fn trigger_dreaming(
    js: &jetstream::Context,
    actor_type: &str,
    actor_key: &str,
    session_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let trigger = DreamTrigger {
        actor_type: actor_type.to_string(),
        actor_key: actor_key.to_string(),
        session_id: session_id.to_string(),
    };
    let payload = serde_json::to_vec(&trigger)?;
    let subject = format!(
        "sessions.dream.{}.{}.{}",
        trogon_transcript::subject::sanitize_key(actor_type),
        trogon_transcript::subject::sanitize_key(actor_key),
        session_id,
    );
    js.publish(subject, payload.into()).await?.await?;
    Ok(())
}

// Expose kv::Store for use in production main.rs wiring
pub use kv::Store as KvStore;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_error_display_get_stream() {
        let e = ServiceError::GetStream("timeout".into());
        assert!(e.to_string().contains("get_stream failed"));
        assert!(e.to_string().contains("timeout"));
    }

    #[test]
    fn service_error_display_create_consumer() {
        let e = ServiceError::CreateConsumer("already exists".into());
        assert!(e.to_string().contains("create_consumer failed"));
    }

    #[test]
    fn service_error_display_messages() {
        let e = ServiceError::Messages("stream closed".into());
        assert!(e.to_string().contains("messages() failed"));
    }

    #[test]
    fn service_error_is_error_trait() {
        let e: Box<dyn std::error::Error> = Box::new(ServiceError::GetStream("x".into()));
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn dream_trigger_serde_round_trip() {
        let trigger = DreamTrigger {
            actor_type: "pr".into(),
            actor_key: "owner/repo/1".into(),
            session_id: "sess-abc".into(),
        };
        let json = serde_json::to_string(&trigger).unwrap();
        let restored: DreamTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.actor_type, "pr");
        assert_eq!(restored.actor_key, "owner/repo/1");
        assert_eq!(restored.session_id, "sess-abc");
    }
}
