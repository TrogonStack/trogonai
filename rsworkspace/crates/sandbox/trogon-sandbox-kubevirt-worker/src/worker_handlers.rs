use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use tracing::warn;
use trogon_sandbox_contract::{
    CheckpointRunningRequest, Digest, LeaseId, OciArtifactRef, ProviderDispatchPayload, ProviderDispatchReply,
    SnapshotExportError, SnapshotExportOutcome,
};
use trogon_sandbox_protocol::{ProviderError, RedispatchGuard, SnapshotError, decode_verified, nats_transfer};
use trogon_sandbox_ssh_gateway::TerminalHub;

use crate::constants::CHECKPOINT_PLACEHOLDER_DIGEST;
use crate::launch_lease::launch_lease;
use crate::{KubeVirtWorkerError, NatsLifecycleEmitter, WorkerProvider};

/// Launches the sandbox a dispatch asks for and answers the broker. A redelivered dispatch gets the first grant
/// instead of a second sandbox.
pub(crate) async fn serve_lease(
    client: Client,
    provider: Arc<dyn WorkerProvider>,
    grants: Arc<RedispatchGuard>,
    message: Message,
) -> Result<(), KubeVirtWorkerError> {
    let reply = match decode_verified::<ProviderDispatchPayload>(&message.payload, None) {
        Err(error) => ProviderDispatchReply::DispatchRejected {
            decode_error: error.to_string(),
        },
        Ok(dispatch) => {
            let emitter = NatsLifecycleEmitter::new(client.clone(), &dispatch.lease_id);
            match grants
                .get_or_launch(&dispatch.lease_id, || launch_lease(&*provider, &emitter, &dispatch))
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

/// Ends the lease named by the last token of the subject. Its terminals close first. The first grant is forgotten
/// only once the provider has ended the lease, so a cancel that failed can be retried against the same lease.
pub(crate) async fn serve_cancel(
    provider: Arc<dyn WorkerProvider>,
    grants: Arc<RedispatchGuard>,
    terminals: Arc<TerminalHub>,
    message: Message,
) -> Result<(), KubeVirtWorkerError> {
    let lease = LeaseId::new(message.subject.rsplit('.').next().unwrap_or_default());
    terminals.close_lease(&lease).await;
    provider.cancel(&lease).await?;
    grants.forget(&lease);
    Ok(())
}

/// Checkpoints the running VM of a lease this worker holds, without stopping it, and pushes the disk to the
/// requested registry. A lease another worker holds gets no answer, so that worker can answer. KubeVirt keeps no
/// provider-native snapshots, so a request for one is refused.
pub(crate) async fn serve_checkpoint(
    client: Client,
    provider: Arc<dyn WorkerProvider>,
    message: Message,
) -> Result<(), KubeVirtWorkerError> {
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let request: CheckpointRunningRequest = serde_json::from_slice(&payload)?;
    if !provider.holds(&request.lease_id).await {
        return Ok(());
    }
    let outcome = checkpoint_outcome(&*provider, &request).await;
    nats_transfer::respond(&client, reply.as_str(), Bytes::from(serde_json::to_vec(&outcome)?)).await?;
    Ok(())
}

async fn checkpoint_outcome(
    provider: &dyn WorkerProvider,
    request: &CheckpointRunningRequest,
) -> SnapshotExportOutcome {
    match checkpoint(provider, request).await {
        Ok(outcome) => outcome,
        Err(outcome) => *outcome,
    }
}

/// The outcome of a checkpoint request. The error side is a refusal, which is also an outcome the broker is told.
async fn checkpoint(
    provider: &dyn WorkerProvider,
    request: &CheckpointRunningRequest,
) -> Result<SnapshotExportOutcome, Box<SnapshotExportOutcome>> {
    if request.native_name.is_some() {
        return Err(Box::new(SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
            message: "KubeVirt snapshots are pushed to a registry; ask for an exported snapshot".to_owned(),
        })));
    }
    #[rustfmt::skip]
    let digest = Digest::new(CHECKPOINT_PLACEHOLDER_DIGEST).map_err(|error| Box::new(SnapshotExportOutcome::Err(SnapshotExportError::Permanent { message: error.to_string() })));
    let digest = digest?;
    let target = OciArtifactRef {
        registry: request.target.registry.clone(),
        repository: request.target.repository.clone(),
        digest,
        artifact_type: request.target.artifact_type,
    };
    Ok(match provider.checkpoint_running(&request.lease_id, &target).await {
        Ok(receipt) => SnapshotExportOutcome::Ok(receipt),
        Err(ProviderError::Snapshot(SnapshotError::Unsupported)) => {
            SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
                message: SnapshotError::Unsupported.to_string(),
            })
        }
        Err(error) => {
            warn!(lease_id = %request.lease_id, %error, "checkpoint failed");
            SnapshotExportOutcome::Err(SnapshotExportError::Transient {
                message: error.to_string(),
            })
        }
    })
}

/// Answers where a declared port can be reached, when this worker holds the lease.
pub(crate) async fn serve_get_domain(
    client: Client,
    provider: Arc<dyn WorkerProvider>,
    message: async_nats::Message,
) -> Result<(), KubeVirtWorkerError> {
    Ok(crate::serve_domain(client, provider, message).await?)
}
