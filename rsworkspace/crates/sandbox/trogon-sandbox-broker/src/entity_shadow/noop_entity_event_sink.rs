use async_trait::async_trait;
use trogon_sandbox_contract::EntityLifecycleEvent;

use super::{EntityEventSink, EntityEventSinkError};

pub struct NoopEntityEventSink;

#[async_trait]
impl EntityEventSink for NoopEntityEventSink {
    async fn publish(&self, _event: &EntityLifecycleEvent) -> Result<(), EntityEventSinkError> {
        Ok(())
    }
}
