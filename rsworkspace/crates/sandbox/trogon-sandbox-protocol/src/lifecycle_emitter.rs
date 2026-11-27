use async_trait::async_trait;
use trogon_sandbox_contract::LifecycleEvent;

/// A sink for lifecycle events.
#[async_trait]
pub trait LifecycleEmitter: Send + Sync {
    /// Publishes `event`, best effort.
    async fn emit(&self, event: LifecycleEvent);
}
