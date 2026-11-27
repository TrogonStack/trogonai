use std::time::Duration;

use futures::StreamExt;
use trogon_sandbox_contract::{LeaseId, LeaseReleased, LeaseReleasedReason};
use trogon_sandbox_protocol::subjects;

use crate::{ClientError, SandboxClient};

impl SandboxClient {
    /// Waits up to `timeout` for the broker to announce that `lease_id` was released.
    pub async fn wait_released(&self, lease_id: &LeaseId, timeout: Duration) -> Result<(), ClientError> {
        self.wait_released_with_reason(lease_id, timeout).await.map(|_| ())
    }

    /// Like [`Self::wait_released`], and returns why the lease ended. An announcement that does not decode reads as
    /// [`LeaseReleasedReason::Completed`].
    pub async fn wait_released_with_reason(
        &self,
        lease_id: &LeaseId,
        timeout: Duration,
    ) -> Result<LeaseReleasedReason, ClientError> {
        let mut raw = self.bus.subscribe(&subjects::lease_released(lease_id)).await?;
        match tokio::time::timeout(timeout, raw.next()).await {
            Ok(Some(message)) => Ok(serde_json::from_slice::<LeaseReleased>(&message.payload)
                .map(|released| released.reason)
                .unwrap_or(LeaseReleasedReason::Completed)),
            Ok(None) | Err(_) => Err(ClientError::Timeout(timeout)),
        }
    }
}
