use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use trogon_sandbox_contract::{
    CheckpointRunningRequest, GetSandboxDomainRequest, LeaseId, ProviderDispatchPayload, ProviderDispatchReply,
    SnapshotExportError, SnapshotExportOutcome,
};
use trogon_sandbox_protocol::{RedispatchGuard, decode_verified, nats_transfer};

use crate::{ModalProvider, ModalWorkerError, NatsLifecycleEmitter};

/// Launches the sandbox a dispatch asks for and answers the broker. A redelivered dispatch gets the first grant
/// instead of a second sandbox.
pub(crate) async fn serve_lease(
    client: Client,
    provider: Arc<ModalProvider>,
    grants: Arc<RedispatchGuard>,
    message: Message,
) -> Result<(), ModalWorkerError> {
    let reply = match decode_verified::<ProviderDispatchPayload>(&message.payload, None) {
        Err(error) => ProviderDispatchReply::DispatchRejected {
            decode_error: error.to_string(),
        },
        Ok(dispatch) => {
            let emitter = NatsLifecycleEmitter::new(client.clone(), &dispatch.lease_id);
            match grants
                .get_or_launch(&dispatch.lease_id, || provider.launch(&emitter, &dispatch))
                .await
            {
                Ok(lease) => ProviderDispatchReply::Granted(lease),
                Err(error) => ProviderDispatchReply::LaunchFailed {
                    provider_error: error.to_string(),
                },
            }
        }
    };
    if let Some(subject) = message.reply {
        client.publish(subject, serde_json::to_vec(&reply)?.into()).await?;
    }
    Ok(())
}

/// Ends the lease named by the last token of the subject.
pub(crate) async fn serve_cancel(
    provider: Arc<ModalProvider>,
    grants: Arc<RedispatchGuard>,
    message: Message,
) -> Result<(), ModalWorkerError> {
    let lease = LeaseId::new(message.subject.rsplit('.').next().unwrap_or_default());
    grants.forget(&lease);
    Ok(provider.cancel_inner(&lease).await?)
}

/// Answers where a declared port can be reached, when this worker holds the lease so another replica can answer
/// otherwise.
pub(crate) async fn serve_domain(
    client: Client,
    provider: Arc<ModalProvider>,
    message: Message,
) -> Result<(), ModalWorkerError> {
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let request: GetSandboxDomainRequest = serde_json::from_slice(&payload)?;
    let Some(response) = provider.get_domain(&request.lease_id, request.port).await else {
        return Ok(());
    };
    nats_transfer::respond(&client, reply.as_str(), Bytes::from(serde_json::to_vec(&response)?)).await?;
    Ok(())
}

/// Snapshots the filesystem when this worker holds the lease. Modal snapshots are provider-native images, so a
/// request without a native name is refused.
pub(crate) async fn serve_checkpoint(
    client: Client,
    provider: Arc<ModalProvider>,
    message: Message,
) -> Result<(), ModalWorkerError> {
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let request: CheckpointRunningRequest = serde_json::from_slice(&payload)?;
    let outcome = if request.native_name.is_some() {
        let Some(outcome) = provider.native_snapshot(&request.lease_id).await else {
            return Ok(());
        };
        outcome
    } else {
        SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
            message: "Modal snapshots are provider-native; ask for a native snapshot".to_owned(),
        })
    };
    nats_transfer::respond(&client, reply.as_str(), Bytes::from(serde_json::to_vec(&outcome)?)).await?;
    Ok(())
}
