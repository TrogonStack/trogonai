use async_nats::Client;
use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, LifecycleEvent};
use trogon_sandbox_protocol::{LifecycleEmitter, lifecycle_subject};

/// Publishes one lease's lifecycle events on its evidence subject. Delivery is best effort.
pub struct NatsLifecycleEmitter {
    client: Client,
    subject: String,
}

impl NatsLifecycleEmitter {
    pub fn new(client: Client, lease_id: &LeaseId) -> Self {
        Self {
            client,
            subject: lifecycle_subject(lease_id),
        }
    }
}

#[async_trait]
impl LifecycleEmitter for NatsLifecycleEmitter {
    async fn emit(&self, event: LifecycleEvent) {
        let Ok(payload) = serde_json::to_vec(&event) else {
            return;
        };
        if let Err(error) = self.client.publish(self.subject.clone(), payload.into()).await {
            tracing::warn!(subject = self.subject, %error, "failed to publish a lifecycle event");
        }
    }
}
