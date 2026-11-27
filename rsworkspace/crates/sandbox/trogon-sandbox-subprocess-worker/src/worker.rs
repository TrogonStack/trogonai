use std::collections::HashMap;
use std::sync::Arc;

use async_nats::jetstream;
use async_nats::{Client, Subject};
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    CheckpointRunningRequest, CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest,
    CreateSandboxUserResponse, FileOpRequest, GetSandboxDomainRequest, LeaseError, LeaseId, LeaseRequest,
    LifecycleEvent, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse, ProviderDispatchPayload, ProviderKind,
    ProviderLease, SandboxId, SandboxPhase, SnapshotExportError, SnapshotExportOutcome, WorkerId,
};
use trogon_sandbox_protocol::subjects::{
    provider_cancel, provider_checkpoint_running, provider_create_group, provider_create_user, provider_dispatch,
    provider_file_op, provider_get_domain, provider_modify_group_membership,
};
use trogon_sandbox_protocol::{
    PhaseTimer, Provider, ProviderError, RedispatchGuard, decode_verified, encode_file_op_response_within,
    lifecycle_subject, nats_transfer, queue_subscribe_verified, subscribe_verified,
};
use trogon_sandbox_subprocess_worker::WorkspaceRegistry;
use trogon_sandbox_subprocess_worker::port_forward::PortForwardRegistry;

mod constants;

use constants::{LEASE_QUEUE, SANDBOX_USERS_UNSUPPORTED_MESSAGE};

type ActiveLeases = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// Why a dispatch got no grant, as the broker reads it from the reply.
#[derive(Serialize)]
enum DispatchFailure {
    #[serde(rename = "decode_error")]
    Decode(String),
    #[serde(rename = "provider_error")]
    Provider(String),
}

/// The subject pattern covering every lease of one per-lease subject family.
fn any_lease() -> LeaseId {
    LeaseId::new(">")
}

pub async fn run(
    client: Client,
    js: jetstream::Context,
    provider: Arc<dyn Provider>,
    workspaces: WorkspaceRegistry,
    port_forwards: PortForwardRegistry,
    cancel: CancellationToken,
    worker_id: WorkerId,
) -> anyhow::Result<()> {
    let kind = ProviderKind::Subprocess;
    let any = any_lease();
    let mut lease_sub = queue_subscribe_verified(&client, provider_dispatch(kind), LEASE_QUEUE).await?;
    let mut cancel_sub = subscribe_verified(&client, provider_cancel(kind, &any)).await?;
    let mut checkpoint_running_sub = subscribe_verified(&client, provider_checkpoint_running(kind, &any)).await?;
    let mut create_user_sub = subscribe_verified(&client, provider_create_user(kind, &any)).await?;
    let mut create_group_sub = subscribe_verified(&client, provider_create_group(kind, &any)).await?;
    let mut modify_group_membership_sub =
        subscribe_verified(&client, provider_modify_group_membership(kind, &any)).await?;
    let mut file_op_sub = subscribe_verified(&client, provider_file_op(kind, &any)).await?;
    let mut get_domain_sub = subscribe_verified(&client, provider_get_domain(kind, &any)).await?;

    let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
    let grants = Arc::new(RedispatchGuard::new());

    info!("worker ready");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("worker shutting down");
                break;
            }
            maybe_msg = lease_sub.next() => {
                let msg = match maybe_msg {
                    Some(m) => m,
                    None => break,
                };

                let client_clone = client.clone();
                let js_clone = js.clone();
                let provider_clone = Arc::clone(&provider);
                let active_clone = Arc::clone(&active);
                let grants_clone = Arc::clone(&grants);
                let lease_worker = worker_id.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_lease_msg(msg, client_clone, js_clone, provider_clone, active_clone, grants_clone, lease_worker).await {
                        warn!(error = %e, "failed to handle lease request");
                    }
                });
            }
            maybe_cancel = cancel_sub.next() => {
                let msg = match maybe_cancel {
                    Some(m) => m,
                    None => break,
                };

                let provider_clone = Arc::clone(&provider);
                let active_clone = Arc::clone(&active);
                let grants_clone = Arc::clone(&grants);

                tokio::spawn(async move {
                    if let Err(e) = handle_cancel_msg(msg, provider_clone, active_clone, grants_clone).await {
                        warn!(error = %e, "failed to handle cancel");
                    }
                });
            }
            maybe_create_user = create_user_sub.next() => {
                let msg = match maybe_create_user {
                    Some(m) => m,
                    None => break,
                };

                let client_clone = client.clone();
                let workspaces_clone = workspaces.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_create_user_msg(msg, client_clone, workspaces_clone).await {
                        warn!(error = %e, "failed to handle create_user request");
                    }
                });
            }
            maybe_checkpoint = checkpoint_running_sub.next() => {
                let msg = match maybe_checkpoint {
                    Some(message) => message,
                    None => break,
                };
                let client = client.clone();
                let workspaces = workspaces.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_checkpoint_running_msg(msg, client, workspaces).await {
                        warn!(%error, "failed to refuse unsupported checkpoint request");
                    }
                });
            }
            maybe_create_group = create_group_sub.next() => {
                let msg = match maybe_create_group {
                    Some(m) => m,
                    None => break,
                };

                let client_clone = client.clone();
                let workspaces_clone = workspaces.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_create_group_msg(msg, client_clone, workspaces_clone).await {
                        warn!(error = %e, "failed to handle create_group request");
                    }
                });
            }
            maybe_modify_group_membership = modify_group_membership_sub.next() => {
                let msg = match maybe_modify_group_membership {
                    Some(m) => m,
                    None => break,
                };

                let client_clone = client.clone();
                let workspaces_clone = workspaces.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_modify_group_membership_msg(msg, client_clone, workspaces_clone).await {
                        warn!(error = %e, "failed to handle modify_group_membership request");
                    }
                });
            }
            maybe_get_domain = get_domain_sub.next() => {
                let msg = match maybe_get_domain {
                    Some(m) => m,
                    None => break,
                };

                let client_clone = client.clone();
                let port_forwards_clone = port_forwards.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_get_domain_msg(msg, client_clone, port_forwards_clone).await {
                        warn!(error = %e, "failed to handle get_domain request");
                    }
                });
            }
            maybe_file_op = file_op_sub.next() => {
                let msg = match maybe_file_op {
                    Some(m) => m,
                    None => break,
                };

                let client_clone = client.clone();
                let workspaces_clone = workspaces.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_file_op_msg(msg, client_clone, workspaces_clone).await {
                        warn!(error = %e, "failed to handle file_op request");
                    }
                });
            }
        }
    }

    Ok(())
}

#[tracing::instrument(skip_all, fields(tenant = %req_ref.tenant, lease_id = %lease_id))]
async fn handle_lease_inner(
    req_ref: &LeaseRequest,
    lease_id: &LeaseId,
    js: &jetstream::Context,
    provider: &Arc<dyn Provider>,
    active: &ActiveLeases,
) -> Result<ProviderLease, ProviderError> {
    let lifecycle_subj = lifecycle_subject(lease_id);
    let lease_sandbox_id = SandboxId::new(lease_id.as_str()).map_err(ProviderError::backend)?;

    publish_lifecycle(
        js,
        &lifecycle_subj,
        &LifecycleEvent::Started {
            sandbox_id: lease_sandbox_id.clone(),
        },
    )
    .await;

    let task_cancel = CancellationToken::new();
    active
        .lock()
        .await
        .insert(lease_id.as_str().to_owned(), task_cancel.clone());

    let provision = PhaseTimer::start(SandboxPhase::Allocate);
    publish_lifecycle(js, &lifecycle_subj, &provision.phase_started()).await;

    let result = provider.lease(req_ref, lease_id).await;

    let out = match &result {
        Ok(ProviderLease { sandbox_id, .. }) => {
            publish_lifecycle(js, &lifecycle_subj, &provision.complete()).await;
            info!(%lease_id, %sandbox_id, tenant = %req_ref.tenant, "sandbox launched");
            publish_lifecycle(
                js,
                &lifecycle_subj,
                &LifecycleEvent::Completed {
                    sandbox_id: sandbox_id.clone(),
                },
            )
            .await;
            result
        }
        Err(error) => {
            publish_lifecycle(js, &lifecycle_subj, &provision.fail(error.to_string())).await;
            warn!(%lease_id, tenant = %req_ref.tenant, %error, "sandbox launch failed");
            publish_lifecycle(
                js,
                &lifecycle_subj,
                &LifecycleEvent::Failed {
                    sandbox_id: lease_sandbox_id,
                    error: error.to_string(),
                },
            )
            .await;
            result
        }
    };

    active.lock().await.remove(lease_id.as_str());
    drop(task_cancel);

    out
}

async fn handle_lease_msg(
    msg: async_nats::Message,
    client: Client,
    js: jetstream::Context,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
    worker_id: WorkerId,
) -> anyhow::Result<()> {
    let payload = match decode_verified::<ProviderDispatchPayload>(&msg.payload, None) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(%error, "invalid dispatch payload");
            publish_reply(&client, msg.reply, &DispatchFailure::Decode(error.to_string())).await;
            return Ok(());
        }
    };

    info!(lease_id = %payload.lease_id, tenant = %payload.request.tenant, "lease accepted");

    // A redelivered dispatch gets the first grant instead of a second process.
    let result = grants
        .get_or_launch(&payload.lease_id, || async {
            let mut grant = handle_lease_inner(&payload.request, &payload.lease_id, &js, &provider, &active).await?;
            grant.worker_id = Some(worker_id);
            Ok::<_, ProviderError>(grant)
        })
        .await;

    match result {
        Ok(grant) => publish_reply(&client, msg.reply, &grant).await,
        Err(error) => publish_reply(&client, msg.reply, &DispatchFailure::Provider(error.to_string())).await,
    }

    Ok(())
}

/// Encodes `body` as JSON and publishes it to the reply subject, if the request had one.
async fn publish_reply(client: &Client, reply: Option<Subject>, body: &impl Serialize) {
    let Some(reply) = reply else {
        return;
    };
    match serde_json::to_vec(body) {
        Ok(encoded) => {
            if let Err(error) = client.publish(reply, encoded.into()).await {
                warn!(%error, "publishing the reply failed");
            }
        }
        Err(error) => warn!(%error, "failed to encode reply"),
    }
}

async fn handle_cancel_msg(
    msg: async_nats::Message,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
) -> anyhow::Result<()> {
    let lease_id = extract_cancel_lease_id(msg.subject.as_str());

    if let Some(lease_id) = lease_id {
        grants.forget(&lease_id);
        if let Some(token) = active.lock().await.get(lease_id.as_str()) {
            token.cancel();
        }
        if let Err(e) = provider.cancel(&lease_id).await {
            warn!(%lease_id, error = %e, "cancel failed");
        } else {
            info!(%lease_id, "lease released");
        }
    }

    Ok(())
}

async fn handle_checkpoint_running_msg(
    message: async_nats::Message,
    client: Client,
    workspaces: WorkspaceRegistry,
) -> anyhow::Result<()> {
    let Some(reply) = message.reply else {
        return Ok(());
    };
    let request: CheckpointRunningRequest = match serde_json::from_slice(&message.payload) {
        Ok(request) => request,
        Err(error) => {
            warn!(%error, "invalid checkpoint_running request");
            return Ok(());
        }
    };
    if message.subject.as_str() != provider_checkpoint_running(ProviderKind::Subprocess, &request.lease_id)
        || !workspaces.contains(&request.lease_id)
    {
        return Ok(());
    }
    let outcome = SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
        message: "the subprocess provider cannot checkpoint a running lease".into(),
    });
    client
        .publish(reply, Bytes::from(serde_json::to_vec(&outcome)?))
        .await?;
    Ok(())
}

/// Handles a broadcast create_user request. Every subprocess worker receives
/// the broadcast; only the one holding `lease_id` in its workspace registry
/// replies, and it always denies per [`SANDBOX_USERS_UNSUPPORTED_MESSAGE`].
async fn handle_create_user_msg(
    msg: async_nats::Message,
    client: Client,
    workspaces: WorkspaceRegistry,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: CreateSandboxUserRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid create_user request");
            return Ok(());
        }
    };

    if msg.subject.as_str() != provider_create_user(ProviderKind::Subprocess, &request.lease_id)
        || !workspaces.contains(&request.lease_id)
    {
        return Ok(());
    }

    let response = CreateSandboxUserResponse::Denied(LeaseError::Backend {
        message: SANDBOX_USERS_UNSUPPORTED_MESSAGE.to_string(),
    });

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast create_group request. Routed like [`handle_create_user_msg`].
async fn handle_create_group_msg(
    msg: async_nats::Message,
    client: Client,
    workspaces: WorkspaceRegistry,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: CreateSandboxGroupRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid create_group request");
            return Ok(());
        }
    };

    if msg.subject.as_str() != provider_create_group(ProviderKind::Subprocess, &request.lease_id)
        || !workspaces.contains(&request.lease_id)
    {
        return Ok(());
    }

    let response = CreateSandboxGroupResponse::Denied(LeaseError::Backend {
        message: SANDBOX_USERS_UNSUPPORTED_MESSAGE.to_string(),
    });

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast modify_group_membership request. Routed like
/// [`handle_create_user_msg`].
async fn handle_modify_group_membership_msg(
    msg: async_nats::Message,
    client: Client,
    workspaces: WorkspaceRegistry,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: ModifyGroupMembershipRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid modify_group_membership request");
            return Ok(());
        }
    };

    if msg.subject.as_str() != provider_modify_group_membership(ProviderKind::Subprocess, &request.lease_id)
        || !workspaces.contains(&request.lease_id)
    {
        return Ok(());
    }

    let response = ModifyGroupMembershipResponse::Denied(LeaseError::Backend {
        message: SANDBOX_USERS_UNSUPPORTED_MESSAGE.to_string(),
    });

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast get_domain request. Every subprocess worker receives
/// it; only the one running `lease_id` replies.
async fn handle_get_domain_msg(
    msg: async_nats::Message,
    client: Client,
    port_forwards: PortForwardRegistry,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: GetSandboxDomainRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid get_domain request");
            return Ok(());
        }
    };

    if !port_forwards.holds(&request.lease_id).await {
        return Ok(());
    }

    let response = port_forwards.resolve(&request.lease_id, request.port).await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast file_op request. Every subprocess worker receives it; only the one
/// holding the lease's workspace replies. The reply goes out through the chunk transport so
/// a large read or archive is not capped by the bus message size.
async fn handle_file_op_msg(
    msg: async_nats::Message,
    client: Client,
    workspaces: WorkspaceRegistry,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };

    let payload = nats_transfer::receive(&client, &msg).await?;
    let request: FileOpRequest = match serde_json::from_slice(&payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid file_op request");
            return Ok(());
        }
    };

    let Some(response) = workspaces.execute(&request).await else {
        return Ok(());
    };

    let body = encode_file_op_response_within(&response, nats_transfer::MAX_TRANSFER_BYTES)?;
    nats_transfer::respond(&client, reply.as_str(), Bytes::from(body)).await?;
    publish_watch_overflows(&client, &workspaces, &request.lease_id).await;
    Ok(())
}

/// Puts watch overflows on the lease's evidence timeline, which a caller that
/// stopped polling would otherwise never learn of. Best effort: a failed
/// publish is logged and never fails the file operation.
async fn publish_watch_overflows(client: &Client, workspaces: &WorkspaceRegistry, lease: &LeaseId) {
    for (subject, evidence) in workspaces.take_overflow_evidence(lease) {
        match serde_json::to_vec(&evidence) {
            Ok(payload) => {
                if let Err(e) = client.publish(subject.clone(), payload.into()).await {
                    warn!(%subject, error = %e, "failed to publish file watch overflow");
                }
            }
            Err(e) => warn!(error = %e, "failed to serialize file watch overflow"),
        }
    }
}

pub(crate) fn extract_cancel_lease_id(subject: &str) -> Option<LeaseId> {
    subject
        .split('.')
        .next_back()
        .filter(|s| !s.is_empty())
        .map(LeaseId::new)
}

async fn publish_lifecycle(js: &jetstream::Context, subject: &str, event: &LifecycleEvent) {
    match serde_json::to_vec(event) {
        Ok(payload) => {
            if let Err(e) = js.publish(subject.to_string(), payload.into()).await {
                warn!(subject, error = %e, "failed to publish lifecycle event");
            }
        }
        Err(e) => {
            warn!(error = %e, "failed to serialize lifecycle event");
        }
    }
}

#[cfg(test)]
mod tests;
