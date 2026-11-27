use async_nats::{Client, Message};
use tracing::warn;
use trogon_sandbox_contract::{SnapshotExportError, SnapshotExportOutcome, SnapshotExportRequest, SnapshotReceipt};

use crate::publish_reply::publish_reply;
use crate::snapshot::export_and_push_rootfs_from_pod;
use crate::{K8sWorkerError, K8sWorkerProvider};

/// Exports the lease a broker names to the snapshot target it names, and replies with the receipt or why it failed.
pub(crate) async fn handle_snapshot_export(client: Client, provider: &K8sWorkerProvider, msg: Message) {
    let request = match serde_json::from_slice::<SnapshotExportRequest>(&msg.payload) {
        Ok(request) => request,
        Err(error) => {
            warn!(%error, "invalid snapshot export request");
            return;
        }
    };
    let outcome = match export_for_lease(provider, &request).await {
        Ok(receipt) => SnapshotExportOutcome::Ok(receipt),
        Err(error @ K8sWorkerError::NoActiveLease(_)) => SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
            message: error.to_string(),
        }),
        Err(error) => SnapshotExportOutcome::Err(SnapshotExportError::Transient {
            message: error.to_string(),
        }),
    };
    publish_reply(&client, msg.reply, &outcome).await;
}

async fn export_for_lease(
    provider: &K8sWorkerProvider,
    request: &SnapshotExportRequest,
) -> Result<SnapshotReceipt, K8sWorkerError> {
    let pod_name = provider
        .sandbox_for_lease(&request.lease_id)
        .await
        .ok_or_else(|| K8sWorkerError::NoActiveLease(request.lease_id.clone()))?;
    let receipt =
        export_and_push_rootfs_from_pod(&provider.inner, provider.oci.as_ref(), &pod_name, &request.target).await?;
    provider.remember_pushed(&request.target, &receipt);
    Ok(receipt)
}
