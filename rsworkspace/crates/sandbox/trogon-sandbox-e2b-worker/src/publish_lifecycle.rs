use async_nats::jetstream;
use tracing::warn;
use trogon_sandbox_contract::LifecycleEvent;

/// Publishes `event` on `subject`. A lifecycle event is telemetry, so a failure is logged and never fails the lease.
pub(crate) async fn publish_lifecycle(js: &jetstream::Context, subject: &str, event: &LifecycleEvent) {
    let sent: Result<(), Box<dyn std::error::Error>> = async {
        let payload = serde_json::to_vec(event)?;
        js.publish(subject.to_owned(), payload.into()).await?;
        Ok(())
    }
    .await;
    if let Err(error) = sent {
        warn!(subject, %error, "failed to publish lifecycle event");
    }
}

#[cfg(test)]
mod tests;
