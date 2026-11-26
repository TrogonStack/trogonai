use async_trait::async_trait;
use trogon_sandbox_contract::LifecycleEvent;

use crate::LifecycleEmitter;

/// An emitter for providers that do not publish lifecycle events.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopLifecycleEmitter;

#[async_trait]
impl LifecycleEmitter for NoopLifecycleEmitter {
    async fn emit(&self, _event: LifecycleEvent) {}
}
