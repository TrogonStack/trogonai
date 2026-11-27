use async_nats::Message;
use trogon_sandbox_contract::{
    CheckpointRunningRequest, SandboxRollbackRequest, SnapshotExportError, SnapshotExportOutcome,
};

use crate::K8sVerb;
use crate::inbound::Inbound;
use crate::verb_context::VerbContext;

/// Exports a running lease to its snapshot target. A failed live checkpoint is retryable: the lease keeps running.
pub(crate) async fn checkpoint_running(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<CheckpointRunningRequest>::decode(&msg, K8sVerb::CheckpointRunning) else {
        return;
    };
    let request = inbound.request;
    if !ctx.provider.holds_lease(&request.lease_id).await {
        return;
    }
    let outcome = match ctx
        .provider
        .checkpoint_for_lease(&request.lease_id, &request.target)
        .await
    {
        Ok(receipt) => SnapshotExportOutcome::Ok(receipt),
        Err(error) => SnapshotExportOutcome::Err(SnapshotExportError::Transient {
            message: error.to_string(),
        }),
    };
    ctx.reply(inbound.reply, &outcome).await;
}

/// Rebuilds a lease's sandbox from a snapshot.
pub(crate) async fn rollback(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<SandboxRollbackRequest>::decode(&msg, K8sVerb::Rollback) else {
        return;
    };
    let request = inbound.request;
    if !ctx.provider.holds_lease(&request.lease_id).await {
        return;
    }
    let response = ctx
        .provider
        .rollback_for_lease(&request.lease_id, &request.snapshot)
        .await;
    ctx.reply(inbound.reply, &response).await;
}
