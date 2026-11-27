use async_nats::jetstream;
use async_trait::async_trait;
use tracing::warn;
use trogon_sandbox_contract::LifecycleEvent;
use trogon_sandbox_protocol::LifecycleEmitter;

pub struct NatsLifecycleEmitter {
    js: jetstream::Context,
    subject: String,
}

impl NatsLifecycleEmitter {
    pub fn new(js: jetstream::Context, subject: String) -> Self {
        Self { js, subject }
    }
}

#[async_trait]
impl LifecycleEmitter for NatsLifecycleEmitter {
    async fn emit(&self, event: LifecycleEvent) {
        match serde_json::to_vec(&event) {
            Ok(payload) => {
                if let Err(e) = self.js.publish(self.subject.clone(), payload.into()).await {
                    warn!(subject = %self.subject, error = %e, "failed to publish lifecycle event");
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to serialize lifecycle event");
            }
        }
    }
}
