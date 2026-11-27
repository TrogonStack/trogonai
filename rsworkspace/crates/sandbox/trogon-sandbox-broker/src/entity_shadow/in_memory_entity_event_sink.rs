use std::sync::Mutex;

use async_trait::async_trait;
use trogon_sandbox_contract::EntityLifecycleEvent;

use super::{EntityEventSink, EntityEventSinkError};

#[derive(Default)]
pub struct InMemoryEntityEventSink {
    events: Mutex<Vec<EntityLifecycleEvent>>,
}

impl InMemoryEntityEventSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<EntityLifecycleEvent> {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[async_trait]
impl EntityEventSink for InMemoryEntityEventSink {
    async fn publish(&self, event: &EntityLifecycleEvent) -> Result<(), EntityEventSinkError> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
        Ok(())
    }
}
