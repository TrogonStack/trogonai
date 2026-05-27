pub mod consumer;
pub mod normalize;

use async_trait::async_trait;

use crate::envelope::AuditEnvelope;
use crate::error::ProjectorError;
use crate::event::TrafficEvent;

/// JetStream durable consumer name (spec section 9).
pub const DURABLE_CONSUMER: &str = "agent-traffic-projector";

pub const DEFAULT_STREAM: &str = "MCP_AUDIT";

pub const STS_FILTER: &str = "mcp.audit.sts.>";

pub const REGISTRY_FILTER: &str = "mcp.audit.registry.>";

#[async_trait]
pub trait AuditProjector: Send + Sync {
    async fn project(&self, envelope: AuditEnvelope) -> Result<(), ProjectorError>;
}

#[async_trait]
pub trait AuditConsumer: Send + Sync {
    async fn run(&self) -> Result<(), ProjectorError>;

    async fn handle_message(&self, subject: &str, payload: &[u8]) -> Result<(), ProjectorError>;
}

pub struct ProjectingConsumer<P: AuditProjector> {
    projector: P,
}

impl<P: AuditProjector> ProjectingConsumer<P> {
    pub fn new(projector: P) -> Self {
        Self { projector }
    }

    pub async fn project_payload(&self, subject: &str, payload: &[u8]) -> Result<TrafficEvent, ProjectorError> {
        let envelope = AuditEnvelope::from_message(subject, payload)?;
        let event = normalize::normalize_envelope(&envelope)?;
        self.projector.project(envelope).await?;
        Ok(event)
    }
}

#[async_trait]
impl<P: AuditProjector + Send + Sync> AuditConsumer for ProjectingConsumer<P> {
    async fn run(&self) -> Result<(), ProjectorError> {
        Err(ProjectorError::Consumer(
            "use projector::consumer::JetStreamAuditConsumer::run".into(),
        ))
    }

    async fn handle_message(&self, subject: &str, payload: &[u8]) -> Result<(), ProjectorError> {
        let envelope = AuditEnvelope::from_message(subject, payload)?;
        self.projector.project(envelope).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::envelope::AuditEnvelope;

    struct RecordingProjector {
        envelopes: Arc<Mutex<Vec<AuditEnvelope>>>,
    }

    #[async_trait]
    impl AuditProjector for RecordingProjector {
        async fn project(&self, envelope: AuditEnvelope) -> Result<(), ProjectorError> {
            self.envelopes.lock().expect("lock").push(envelope);
            Ok(())
        }
    }

    #[tokio::test]
    async fn audit_projector_receives_envelope() {
        let store = Arc::new(Mutex::new(Vec::new()));
        let consumer = ProjectingConsumer::new(RecordingProjector {
            envelopes: Arc::clone(&store),
        });
        consumer
            .handle_message(
                "mcp.audit.sts.success",
                br#"{"event_id":"evt-1","outcome":"success","tenant":"acme"}"#,
            )
            .await
            .expect("handle");
        assert_eq!(store.lock().expect("lock").len(), 1);
    }
}
