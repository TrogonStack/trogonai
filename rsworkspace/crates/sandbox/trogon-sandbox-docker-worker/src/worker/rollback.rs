use std::sync::Arc;

use crate::adapter::DockerWorkerProvider;
use async_nats::Client;
use bytes::Bytes;
use tracing::warn;
use trogon_sandbox_contract::SandboxRollbackRequest;

use super::ActiveLeases;

/// Handles a broadcast rollback request. Routed like `handle_freeze_msg`:
/// only the worker holding `lease_id` replies. A worker that restarted holds
/// no active lease, but replies when it kept the lease's request on disk.
pub(super) async fn handle_rollback_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: SandboxRollbackRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid rollback request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) && !provider.has_rollback_state(&request.lease_id).await {
        return Ok(());
    }

    let response = provider.rollback_for_lease(&request.lease_id, &request.snapshot).await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}
