use std::pin::Pin;

use async_nats::HeaderMap;
use bytes::Bytes;

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::audit::envelope::AuditEnvelope;
use crate::audit::task_lifecycle::TaskLifecycleEnvelope;

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

pub trait AuditEmitter: Send + Sync {
    /// Published on `{prefix}.v1.audit.{agent_id}.{ok|err}` per ADR#0055's
    /// operational profile: entity-scoped, fixed terminals. The JSON-RPC method
    /// travels in the payload rather than as a subject token so the subject
    /// vocabulary does not grow with the method set.
    fn publish<'a>(
        &'a self,
        prefix: &'a A2aPrefix,
        agent_id: &'a A2aAgentId,
        envelope: AuditEnvelope,
    ) -> BoxFuture<'a, ()>;

    /// Published on `{prefix}.v1.audit.{agent_id}.lifecycle` for each
    /// [`TaskLifecycleEnvelope`] emitted from the streaming task pump (`message/stream`).
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
            let subject = format!("{}.v1.audit.{}.{}", prefix.as_str(), agent_id.as_str(), outcome_token);
            let payload = Bytes::from(serde_json::to_vec(&envelope).unwrap_or_default());
            if let Err(e) = self
                .nats
                .publish_with_headers(async_nats::Subject::from(subject.as_str()), HeaderMap::new(), payload)
                .await
            {
                tracing::warn!(error = %e, "failed to publish audit envelope");
            }
        })
    }

    fn publish_task_lifecycle<'a>(
        &'a self,
        prefix: &'a A2aPrefix,
        agent_id: &'a A2aAgentId,
        envelope: TaskLifecycleEnvelope,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let subject = format!("{}.v1.audit.{}.lifecycle", prefix.as_str(), agent_id.as_str());
            let payload = Bytes::from(serde_json::to_vec(&envelope).unwrap_or_default());
            if let Err(e) = self
                .nats
                .publish_with_headers(async_nats::Subject::from(subject.as_str()), HeaderMap::new(), payload)
                .await
            {
                tracing::warn!(error = %e, "failed to publish task lifecycle audit envelope");
            }
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
mod tests;
