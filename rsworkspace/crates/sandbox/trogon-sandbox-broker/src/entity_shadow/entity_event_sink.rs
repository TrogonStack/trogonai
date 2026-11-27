use async_trait::async_trait;
use trogon_sandbox_contract::EntityLifecycleEvent;

use crate::entity_event_sink_error::EntityEventSinkError;

/// Destination of entity lifecycle events. Publishing is best effort.
#[async_trait]
pub trait EntityEventSink: Send + Sync {
    async fn publish(&self, event: &EntityLifecycleEvent) -> Result<(), EntityEventSinkError>;
}
