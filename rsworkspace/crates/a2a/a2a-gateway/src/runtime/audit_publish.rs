//! Fire-and-forget audit envelope publishing.
//!
//! The dispatch path calls this once per ingress request, after the
//! outcome (allow / deny / forward) is decided. Publishing is async
//! and best-effort: a failure to publish must not block or fail the
//! ingress request itself, so we spawn a detached task and log
//! warnings rather than propagating errors back.

use a2a_nats::A2aPrefix;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::audit::emitter::{AuditEmitter, NatsAuditEmitter};
use a2a_nats::audit::envelope::AuditEnvelope;
use trogon_nats::PublishClient;

/// Spawn a detached task that publishes an audit envelope onto the
/// audit subject derived from `prefix`, `agent_id`, and the envelope
/// method+outcome. When `enabled` is `false` the call is a no-op so
/// deployments without an audit stream pay no publish cost.
///
/// Fire-and-forget by design: dispatch returns immediately, and a
/// publish failure surfaces only as a `tracing::warn` from the
/// emitter. Callers MUST NOT depend on the envelope being durably
/// stored before they reply to the ingress request.
pub fn spawn_gateway_audit_publish<N>(
    enabled: bool,
    client: N,
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
    envelope: AuditEnvelope,
) where
    N: PublishClient,
{
    if !enabled {
        return;
    }
    tokio::spawn(async move {
        let emitter = NatsAuditEmitter::new(client);
        emitter.publish(&prefix, &agent_id, envelope).await;
    });
}

#[cfg(test)]
mod tests;
