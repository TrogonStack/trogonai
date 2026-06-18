use std::pin::Pin;

use async_nats::HeaderMap;
use bytes::Bytes;

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::audit::envelope::AuditEnvelope;
use crate::audit::task_lifecycle::TaskLifecycleEnvelope;

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

pub trait AuditEmitter: Send + Sync {
    fn publish<'a>(
        &'a self,
        prefix: &'a A2aPrefix,
        agent_id: &'a A2aAgentId,
        envelope: AuditEnvelope,
    ) -> BoxFuture<'a, ()>;

    /// Published on each [`TaskLifecycleEnvelope`] emitted from the streaming task pump (`message/stream`).
    fn publish_task_lifecycle<'a>(
        &'a self,
        prefix: &'a A2aPrefix,
        agent_id: &'a A2aAgentId,
        envelope: TaskLifecycleEnvelope,
    ) -> BoxFuture<'a, ()>;
}

pub struct NatsAuditEmitter<N> {
    nats: N,
}

impl<N> NatsAuditEmitter<N> {
    pub fn new(nats: N) -> Self {
        Self { nats }
    }
}

impl<N> AuditEmitter for NatsAuditEmitter<N>
where
    N: trogon_nats::PublishClient,
{
    fn publish<'a>(
        &'a self,
        prefix: &'a A2aPrefix,
        agent_id: &'a A2aAgentId,
        envelope: AuditEnvelope,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let outcome_token = match &envelope.outcome {
                crate::audit::envelope::AuditOutcome::Ok => "ok",
                crate::audit::envelope::AuditOutcome::Err { .. } => "err",
            };
            let subject = format!(
                "{}.audit.{}.{}",
                prefix.as_str(),
                outcome_token,
                envelope.method.replace('/', ".")
            );
            let payload = Bytes::from(serde_json::to_vec(&envelope).unwrap_or_default());
            if let Err(e) = self
                .nats
                .publish_with_headers(async_nats::Subject::from(subject.as_str()), HeaderMap::new(), payload)
                .await
            {
                tracing::warn!(error = %e, "failed to publish audit envelope");
            }
            let _ = agent_id;
        })
    }

    fn publish_task_lifecycle<'a>(
        &'a self,
        prefix: &'a A2aPrefix,
        agent_id: &'a A2aAgentId,
        envelope: TaskLifecycleEnvelope,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let subject = format!("{}.audit.lifecycle", prefix.as_str());
            let payload = Bytes::from(serde_json::to_vec(&envelope).unwrap_or_default());
            if let Err(e) = self
                .nats
                .publish_with_headers(async_nats::Subject::from(subject.as_str()), HeaderMap::new(), payload)
                .await
            {
                tracing::warn!(error = %e, "failed to publish task lifecycle audit envelope");
            }
            let _ = agent_id;
        })
    }
}

pub struct NoopAuditEmitter;

impl AuditEmitter for NoopAuditEmitter {
    fn publish<'a>(
        &'a self,
        _prefix: &'a A2aPrefix,
        _agent_id: &'a A2aAgentId,
        _envelope: AuditEnvelope,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn publish_task_lifecycle<'a>(
        &'a self,
        _prefix: &'a A2aPrefix,
        _agent_id: &'a A2aAgentId,
        _envelope: TaskLifecycleEnvelope,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::envelope::AuditOutcome;
    use trogon_nats::AdvancedMockNatsClient;

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a").unwrap()
    }

    fn agent() -> A2aAgentId {
        A2aAgentId::new("bot").unwrap()
    }

    fn make_envelope(outcome: AuditOutcome) -> AuditEnvelope {
        AuditEnvelope::new(
            &agent(),
            "message/send",
            Some("r1".into()),
            1000,
            5,
            outcome,
            None,
            crate::audit::envelope::AuditEnvelopeFields::default(),
        )
    }

    #[tokio::test]
    async fn nats_emitter_publishes_ok_subject() {
        let nats = AdvancedMockNatsClient::new();
        let emitter = NatsAuditEmitter::new(nats.clone());
        emitter
            .publish(&prefix(), &agent(), make_envelope(AuditOutcome::Ok))
            .await;
        assert_eq!(nats.published_messages(), vec!["a2a.audit.ok.message.send"]);
    }

    #[tokio::test]
    async fn nats_emitter_publishes_err_subject() {
        let nats = AdvancedMockNatsClient::new();
        let emitter = NatsAuditEmitter::new(nats.clone());
        emitter
            .publish(
                &prefix(),
                &agent(),
                make_envelope(AuditOutcome::Err {
                    code: -32001,
                    message: "oops".into(),
                }),
            )
            .await;
        assert_eq!(nats.published_messages(), vec!["a2a.audit.err.message.send"]);
    }

    #[tokio::test]
    async fn nats_emitter_payload_is_valid_json() {
        let nats = AdvancedMockNatsClient::new();
        let emitter = NatsAuditEmitter::new(nats.clone());
        emitter
            .publish(&prefix(), &agent(), make_envelope(AuditOutcome::Ok))
            .await;
        let payloads = nats.published_payloads();
        let v: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
        assert_eq!(v["agent_id"], "bot");
        assert_eq!(v["method"], "message/send");
    }

    #[tokio::test]
    async fn nats_emitter_task_lifecycle_publishes_lifecycle_subject() {
        let nats = AdvancedMockNatsClient::new();
        let emitter = NatsAuditEmitter::new(nats.clone());
        let env = crate::audit::task_lifecycle::TaskLifecycleEnvelope::new(
            &agent(),
            "task-xyz",
            Some("rpc-77".into()),
            a2a::types::TaskState::Submitted,
            a2a::types::TaskState::Working,
            4000,
        );
        emitter.publish_task_lifecycle(&prefix(), &agent(), env).await;
        assert_eq!(nats.published_messages(), vec!["a2a.audit.lifecycle"]);
        let payloads = nats.published_payloads();
        let v: serde_json::Value = serde_json::from_slice(&payloads[0]).unwrap();
        assert_eq!(v["task_id"], "task-xyz");
        assert!(v["prev_task_state"].is_string());
        assert!(v["new_task_state"].is_string());
        assert_eq!(v["agent_id"], "bot");
        assert_eq!(v["json_rpc_req_id"], "rpc-77");
    }

    #[tokio::test]
    async fn noop_emitter_does_not_publish() {
        let emitter = NoopAuditEmitter;
        emitter
            .publish(&prefix(), &agent(), make_envelope(AuditOutcome::Ok))
            .await;
    }

    #[tokio::test]
    async fn noop_emitter_task_lifecycle_does_not_publish() {
        let emitter = NoopAuditEmitter;
        let env = crate::audit::task_lifecycle::TaskLifecycleEnvelope::new(
            &agent(),
            "task-xyz",
            None,
            a2a::types::TaskState::Submitted,
            a2a::types::TaskState::Working,
            1,
        );
        emitter.publish_task_lifecycle(&prefix(), &agent(), env).await;
    }

    #[tokio::test]
    async fn nats_emitter_swallows_publish_failure() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let emitter = NatsAuditEmitter::new(nats.clone());
        emitter
            .publish(&prefix(), &agent(), make_envelope(AuditOutcome::Ok))
            .await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn nats_emitter_swallows_task_lifecycle_publish_failure() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let emitter = NatsAuditEmitter::new(nats.clone());
        let env = crate::audit::task_lifecycle::TaskLifecycleEnvelope::new(
            &agent(),
            "task-xyz",
            None,
            a2a::types::TaskState::Submitted,
            a2a::types::TaskState::Working,
            1,
        );
        emitter.publish_task_lifecycle(&prefix(), &agent(), env).await;
        assert!(nats.published_messages().is_empty());
    }
}
