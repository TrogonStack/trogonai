use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, pull},
    kv,
};
use futures_util::StreamExt as _;
use tracing::{info, warn};
use trogon_transcript::store::TranscriptStore;

use crate::{
    evaluator::Evaluator,
    provider::EvaluationProvider,
    provision::EVALUATIONS_STREAM,
    store::OutcomesStore,
    types::EvaluateTrigger,
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

/// Subscribes to `sessions.evaluate.>` and runs the evaluator for each trigger.
pub struct EvaluationService<P: EvaluationProvider, S: OutcomesStore> {
    js: jetstream::Context,
    consumer_name: String,
    evaluator: Evaluator<P, S>,
}

impl<P: EvaluationProvider, S: OutcomesStore> EvaluationService<P, S> {
    pub fn new(js: jetstream::Context, consumer_name: String, evaluator: Evaluator<P, S>) -> Self {
        Self { js, consumer_name, evaluator }
    }

    pub async fn run(self) -> Result<(), ServiceError> {
        let stream = self
            .js
            .get_stream(EVALUATIONS_STREAM)
            .await
            .map_err(|e| ServiceError::GetStream(e.to_string()))?;

        let consumer = stream
            .create_consumer(pull::Config {
                durable_name: Some(self.consumer_name.clone()),
                filter_subject: "sessions.evaluate.>".to_string(),
                ack_policy: AckPolicy::Explicit,
                deliver_policy: DeliverPolicy::New,
                ..Default::default()
            })
            .await
            .map_err(|e| ServiceError::CreateConsumer(e.to_string()))?;

        let mut messages = consumer
            .messages()
            .await
            .map_err(|e| ServiceError::Messages(e.to_string()))?;

        info!(consumer = %self.consumer_name, "Evaluation service listening for session completions");

        while let Some(result) = messages.next().await {
            let msg = match result {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, "Error receiving evaluation trigger");
                    continue;
                }
            };

            let trigger: EvaluateTrigger = match serde_json::from_slice(&msg.payload) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "Malformed evaluation trigger payload, skipping");
                    msg.ack().await.ok();
                    continue;
                }
            };

            info!(
                actor_type = %trigger.actor_type,
                actor_key  = %trigger.actor_key,
                session_id = %trigger.session_id,
                rubrics    = trigger.rubric_ids.len(),
                "Processing evaluation trigger"
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
                .evaluator
                .evaluate_session(
                    &trigger.actor_type,
                    &trigger.actor_key,
                    &trigger.session_id,
                    &transcript,
                    &trigger.rubric_ids,
                )
                .await
            {
                Ok(results) => {
                    info!(
                        actor_type = %trigger.actor_type,
                        session_id = %trigger.session_id,
                        evaluations = results.len(),
                        passed      = results.iter().filter(|r| r.passed).count(),
                        failed      = results.iter().filter(|r| !r.passed).count(),
                        "Evaluation complete"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Evaluation service error for session");
                }
            }

            msg.ack().await.ok();
        }

        Ok(())
    }
}

/// Publish an evaluation trigger so the service picks it up after a session ends.
///
/// Pass `rubric_ids = &[]` to run all applicable rubrics automatically.
pub async fn trigger_evaluation(
    js: &jetstream::Context,
    actor_type: &str,
    actor_key: &str,
    session_id: &str,
    rubric_ids: &[String],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let trigger = EvaluateTrigger {
        actor_type: actor_type.to_string(),
        actor_key: actor_key.to_string(),
        session_id: session_id.to_string(),
        rubric_ids: rubric_ids.to_vec(),
    };
    let payload = serde_json::to_vec(&trigger)?;
    let subject = format!(
        "sessions.evaluate.{}.{}.{}",
        trogon_transcript::subject::sanitize_key(actor_type),
        trogon_transcript::subject::sanitize_key(actor_key),
        session_id,
    );
    js.publish(subject, payload.into()).await?.await?;
    Ok(())
}

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
    fn evaluate_trigger_serde_round_trip() {
        let trigger = EvaluateTrigger {
            actor_type: "pr".into(),
            actor_key: "owner/repo/1".into(),
            session_id: "sess-xyz".into(),
            rubric_ids: vec!["r1".into(), "r2".into()],
        };
        let json = serde_json::to_string(&trigger).unwrap();
        let restored: EvaluateTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.actor_type, "pr");
        assert_eq!(restored.rubric_ids, vec!["r1", "r2"]);
    }

    #[test]
    fn evaluate_trigger_empty_rubric_ids_round_trips() {
        let trigger = EvaluateTrigger {
            actor_type: "issue".into(),
            actor_key: "owner/repo/42".into(),
            session_id: "sess-1".into(),
            rubric_ids: vec![],
        };
        let json = serde_json::to_string(&trigger).unwrap();
        let restored: EvaluateTrigger = serde_json::from_str(&json).unwrap();
        assert!(restored.rubric_ids.is_empty());
    }
}
