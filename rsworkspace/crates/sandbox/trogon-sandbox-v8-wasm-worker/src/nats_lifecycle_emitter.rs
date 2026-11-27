use async_nats::jetstream;
use async_trait::async_trait;
use tracing::warn;
use trogon_sandbox_contract::LifecycleEvent;
use trogon_sandbox_protocol::LifecycleEmitter;

/// Publishes a lease's lifecycle events to its JetStream subject, best effort.
pub struct NatsLifecycleEmitter {
    js: jetstream::Context,
    subject: String,
}

impl NatsLifecycleEmitter {
    /// An emitter that publishes through `js` on `subject`.
    pub fn new(js: jetstream::Context, subject: String) -> Self {
        Self { js, subject }
    }
}

#[async_trait]
impl LifecycleEmitter for NatsLifecycleEmitter {
    async fn emit(&self, event: LifecycleEvent) {
        let published = async {
            let payload = serde_json::to_vec(&event)?;
            self.js.publish(self.subject.clone(), payload.into()).await?;
            Ok::<(), Box<dyn std::error::Error>>(())
        };
        let result = published.await;
        result
            .inspect_err(|error| warn!(subject = %self.subject, %error, "failed to publish lifecycle event"))
            .ok();
    }
}
