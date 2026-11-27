use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_nats::Client;
use async_nats::jetstream;
use bytes::Bytes;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::SnapshotExportOutcome;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CheckpointRunningRequest, CommandExecution,
    CreateSandboxGroupRequest, CreateSandboxUserRequest, GetSandboxDomainRequest, KillCommandRequest, LeaseError,
    LeaseId, ListProcessesRequest, ModifyGroupMembershipRequest, NetworkPolicyUpdateRequest,
    NetworkPolicyUpdateResponse, PollCommandRequest, PollCommandResponse, ProviderDispatchPayload, ReadFileRequest,
    ResourceUsageRequest, RestartCommandRequest, RunCommandRequest, RunCommandResponse, SandboxFreezeRequest,
    SandboxId, SnapshotExportError, SnapshotExportRequest, StopCommandRequest, WaitReadyRequest, WriteFilesRequest,
    WriteFilesResponse, WriteStdinRequest,
};
use trogon_sandbox_contract::{LifecycleEvent, ProviderLease, SandboxPhase};
use trogon_sandbox_protocol::RedispatchGuard;
use trogon_sandbox_protocol::subjects::worker_snapshot_export;
use trogon_sandbox_protocol::{
    AuthorityHostError, ProviderError, VerifiedRequest, queue_subscribe_verified, subscribe_verified,
    subscribe_verified_context,
};
use trogon_sandbox_protocol::{PhaseTimer, Provider, lifecycle_subject};
use trogon_sandbox_ssh_gateway::{SshAttachRequest, SshDetachRequest};
use trogon_std::env::SystemEnv;

use crate::adapter::DockerWorkerProvider;
use crate::terminal::DockerTerminals;

mod constants;
mod decode_error_reply;
mod encode_within;
mod targeted;
mod workspace;
use constants::*;
use decode_error_reply::DecodeErrorReply;
use encode_within::{encode_poll_within, encode_run_within};
use workspace::*;

type ActiveLeases = Arc<Mutex<HashMap<LeaseId, CancellationToken>>>;

struct LeaseWorkerContext {
    worker_id: trogon_sandbox_contract::WorkerId,
    current_leases: Arc<AtomicU32>,
    docker_provider: Option<Arc<DockerWorkerProvider>>,
}

pub async fn run(
    client: Client,
    js: jetstream::Context,
    provider: Arc<dyn Provider>,
    docker_provider: Arc<DockerWorkerProvider>,
    worker_id: String,
    cancel: CancellationToken,
    current_leases: Arc<AtomicU32>,
) -> anyhow::Result<()> {
    let mut lease_sub = queue_subscribe_verified(&client, LEASE_SUBJECT, LEASE_QUEUE).await?;
    let mut cancel_sub = client.subscribe(CANCEL_SUBJECT).await?;
    let mut checkpoint_running_sub = subscribe_verified(&client, CHECKPOINT_RUNNING_SUBJECT).await?;
    let mut network_policy_update_sub = subscribe_verified(&client, NETWORK_POLICY_UPDATE_SUBJECT).await?;
    let authority_worker = trogon_sandbox_contract::WorkerId::new(worker_id)
        .map_err(|error| anyhow::anyhow!("invalid worker id: {error}"))?;
    let mut snapshot_sub = subscribe_verified(&client, worker_snapshot_export(&authority_worker)).await?;
    let mut run_command_sub = subscribe_verified_context(&client, RUN_COMMAND_SUBJECT).await?;
    let mut cancel_command_launch_sub = subscribe_verified_context(&client, CANCEL_COMMAND_LAUNCH_SUBJECT).await?;
    let mut poll_command_sub = subscribe_verified(&client, POLL_COMMAND_SUBJECT).await?;
    let mut kill_command_sub = subscribe_verified(&client, KILL_COMMAND_SUBJECT).await?;
    let mut stop_command_sub = subscribe_verified(&client, STOP_COMMAND_SUBJECT).await?;
    let mut restart_command_sub = subscribe_verified(&client, RESTART_COMMAND_SUBJECT).await?;
    let mut list_processes_sub = subscribe_verified(&client, LIST_PROCESSES_SUBJECT).await?;
    let mut wait_ready_sub = subscribe_verified(&client, WAIT_READY_SUBJECT).await?;
    let mut write_stdin_sub = subscribe_verified(&client, WRITE_STDIN_SUBJECT).await?;
    let mut write_files_sub = subscribe_verified(&client, WRITE_FILES_SUBJECT).await?;
    let mut read_file_sub = subscribe_verified(&client, READ_FILE_SUBJECT).await?;
    let mut file_op_sub = subscribe_verified(&client, FILE_OP_SUBJECT).await?;
    let mut create_user_sub = subscribe_verified(&client, CREATE_USER_SUBJECT).await?;
    let mut create_group_sub = subscribe_verified(&client, CREATE_GROUP_SUBJECT).await?;
    let mut modify_group_membership_sub = subscribe_verified(&client, MODIFY_GROUP_MEMBERSHIP_SUBJECT).await?;
    let mut ssh_attach_sub = subscribe_verified(&client, SSH_ATTACH_SUBJECT).await?;
    let mut ssh_detach_sub = subscribe_verified(&client, SSH_DETACH_SUBJECT).await?;
    let mut get_domain_sub = subscribe_verified(&client, GET_DOMAIN_SUBJECT).await?;
    let mut terminal_sub = subscribe_verified(&client, TERMINAL_SUBJECT).await?;
    let mut pause_sub = subscribe_verified(&client, PAUSE_SUBJECT).await?;
    let mut resume_sub = subscribe_verified(&client, RESUME_SUBJECT).await?;
    let mut rollback_sub = subscribe_verified(&client, ROLLBACK_SUBJECT).await?;
    let mut resource_usage_sub = subscribe_verified(&client, RESOURCE_USAGE_SUBJECT).await?;
    let mut native_handle_sub = subscribe_verified(&client, NATIVE_HANDLE_SUBJECT).await?;

    let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
    let grants = Arc::new(RedispatchGuard::new());
    let terminals = Arc::new(DockerTerminals::default());
    let ssh_bridge = trogon_sandbox_ssh_gateway::SshStreamBridge::new(
        Arc::new(DockerSshHost {
            provider: Arc::clone(&docker_provider),
            client: client.clone(),
            terminals: Arc::clone(&terminals),
            active: Arc::clone(&active),
        }),
        client.clone(),
        trogon_sandbox_ssh_gateway::BridgeLimits::default(),
    );

    let authority_mode = trogon_sandbox_protocol::AuthorityMode::from_env(&SystemEnv)?;
    let authority_guard = trogon_sandbox_protocol::AuthorityGuard::new(
        trogon_sandbox_protocol::AuthorityConfig::new(authority_mode),
        authority_worker.clone(),
        Arc::new(trogon_sandbox_protocol::NatsAuthorityOracle::new(
            client.clone(),
            authority_worker.clone(),
        )),
        Arc::new(DockerAuthorityHost {
            docker_provider: Arc::clone(&docker_provider),
            active: Arc::clone(&active),
            grants: Arc::clone(&grants),
            terminals: Arc::clone(&terminals),
            client: client.clone(),
        }),
    );
    tokio::spawn(authority_guard.run(cancel.clone()));

    targeted::serve(
        &client,
        &docker_provider,
        &active,
        &grants,
        &terminals,
        &current_leases,
        &cancel,
    )
    .await?;
    client.flush().await?;
    docker_provider.mark_intake_ready();

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
                let current_leases_clone = Arc::clone(&current_leases);
                let lease_worker = authority_worker.clone();
                let docker_provider = Arc::clone(&docker_provider);

                tokio::spawn(async move {
                    if let Err(e) = handle_lease_msg(msg, client_clone, js_clone, provider_clone, active_clone, grants_clone,
                        LeaseWorkerContext { worker_id: lease_worker, current_leases: current_leases_clone, docker_provider: Some(docker_provider) }).await {
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
                let terminals_clone = Arc::clone(&terminals);
                let client_clone = client.clone();
                let docker_provider = Arc::clone(&docker_provider);

                tokio::spawn(async move {
                    if let Err(e) = handle_cancel_msg(msg, provider_clone, docker_provider, active_clone, grants_clone, terminals_clone, client_clone).await {
                        warn!(error = %e, "failed to handle cancel");
                    }
                });
            }
            maybe_checkpoint = checkpoint_running_sub.next() => {
                let msg = match maybe_checkpoint {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_checkpoint_running_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle checkpoint_running request");
                    }
                });
            }
            maybe_network_policy_update = network_policy_update_sub.next() => {
                let msg = match maybe_network_policy_update {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_network_policy_update_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle network_policy_update request");
                    }
                });
            }
            maybe_snapshot = snapshot_sub.next() => {
                let msg = match maybe_snapshot {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_snapshot_export_msg(msg, client_clone, docker).await {
                        warn!(error = %e, "failed to handle snapshot export");
                    }
                });
            }
            maybe_run_command = run_command_sub.next() => {
                let msg = match maybe_run_command {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_run_command_verified(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle run_command request");
                    }
                });
            }
            maybe_cancel_command_launch = cancel_command_launch_sub.next() => {
                let Some(message) = maybe_cancel_command_launch else { break; };
                let docker = Arc::clone(&docker_provider);
                let client = client.clone();
                let active = Arc::clone(&active);
                tokio::spawn(async move {
                    if let Err(error) = handle_cancel_command_launch(message, client, docker, active).await {
                        warn!(%error, "failed to handle command launch cancellation");
                    }
                });
            }
            maybe_poll_command = poll_command_sub.next() => {
                let msg = match maybe_poll_command {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_poll_command_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle poll_command request");
                    }
                });
            }
            maybe_kill_command = kill_command_sub.next() => {
                let msg = match maybe_kill_command {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_kill_command_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle kill_command request");
                    }
                });
            }
            maybe_stop_command = stop_command_sub.next() => {
                let msg = match maybe_stop_command {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_stop_command_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle stop_command request");
                    }
                });
            }
            maybe_restart_command = restart_command_sub.next() => {
                let msg = match maybe_restart_command {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_restart_command_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle restart_command request");
                    }
                });
            }
            maybe_list_processes = list_processes_sub.next() => {
                let msg = match maybe_list_processes {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_list_processes_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle list_processes request");
                    }
                });
            }
            maybe_wait_ready = wait_ready_sub.next() => {
                let msg = match maybe_wait_ready {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_wait_ready_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle wait_ready request");
                    }
                });
            }
            maybe_write_stdin = write_stdin_sub.next() => {
                let msg = match maybe_write_stdin {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_write_stdin_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle write_stdin request");
                    }
                });
            }
            maybe_write_files = write_files_sub.next() => {
                let msg = match maybe_write_files {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_write_files_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle write_files request");
                    }
                });
            }
            maybe_read_file = read_file_sub.next() => {
                let msg = match maybe_read_file {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_read_file_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle read_file request");
                    }
                });
            }
            maybe_file_op = file_op_sub.next() => {
                let msg = match maybe_file_op {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_file_op_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle file_op request");
                    }
                });
            }
            maybe_create_user = create_user_sub.next() => {
                let msg = match maybe_create_user {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_create_user_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle create_user request");
                    }
                });
            }
            maybe_create_group = create_group_sub.next() => {
                let msg = match maybe_create_group {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_create_group_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle create_group request");
                    }
                });
            }
            maybe_modify_group_membership = modify_group_membership_sub.next() => {
                let msg = match maybe_modify_group_membership {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_modify_group_membership_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle modify_group_membership request");
                    }
                });
            }
            maybe_ssh_attach = ssh_attach_sub.next() => {
                let msg = match maybe_ssh_attach {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let bridge = Arc::clone(&ssh_bridge);

                tokio::spawn(async move {
                    if let Err(e) = handle_ssh_attach_msg(msg, client_clone, docker, bridge).await {
                        warn!(error = %e, "failed to handle ssh attach request");
                    }
                });
            }
            maybe_ssh_detach = ssh_detach_sub.next() => {
                let msg = match maybe_ssh_detach {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let bridge = Arc::clone(&ssh_bridge);

                tokio::spawn(async move {
                    if let Err(e) = handle_ssh_detach_msg(msg, client_clone, docker, bridge).await {
                        warn!(error = %e, "failed to handle ssh detach request");
                    }
                });
            }
            maybe_terminal = terminal_sub.next() => {
                let msg = match maybe_terminal {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);
                let terminals_clone = Arc::clone(&terminals);

                tokio::spawn(async move {
                    if let Err(e) = handle_terminal_msg(msg, client_clone, docker, active_clone, terminals_clone).await {
                        warn!(error = %e, "failed to handle terminal request");
                    }
                });
            }
            maybe_rollback = rollback_sub.next() => {
                let msg = match maybe_rollback {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_rollback_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle rollback request");
                    }
                });
            }
            maybe_pause = pause_sub.next() => {
                let msg = match maybe_pause {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_freeze_msg(msg, client_clone, docker, active_clone, true).await {
                        warn!(error = %e, "failed to handle pause request");
                    }
                });
            }
            maybe_resume = resume_sub.next() => {
                let msg = match maybe_resume {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_freeze_msg(msg, client_clone, docker, active_clone, false).await {
                        warn!(error = %e, "failed to handle resume request");
                    }
                });
            }
            maybe_usage = resource_usage_sub.next() => {
                let msg = match maybe_usage {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_resource_usage_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle resource usage request");
                    }
                });
            }
            maybe_native_handle = native_handle_sub.next() => {
                let msg = match maybe_native_handle {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_native_handle_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle native_handle request");
                    }
                });
            }
            maybe_get_domain = get_domain_sub.next() => {
                let msg = match maybe_get_domain {
                    Some(m) => m,
                    None => break,
                };

                let docker = Arc::clone(&docker_provider);
                let client_clone = client.clone();
                let active_clone = Arc::clone(&active);

                tokio::spawn(async move {
                    if let Err(e) = handle_get_domain_msg(msg, client_clone, docker, active_clone).await {
                        warn!(error = %e, "failed to handle get_domain request");
                    }
                });
            }
        }
    }

    Ok(())
}

#[tracing::instrument(skip_all, fields(tenant = %req_ref.tenant, lease_id))]
async fn handle_lease_inner(
    req_ref: &trogon_sandbox_contract::LeaseRequest,
    lease_id: &LeaseId,
    js: &jetstream::Context,
    provider: &Arc<dyn Provider>,
    active: &ActiveLeases,
) -> Result<ProviderLease, ProviderError> {
    tracing::Span::current().record(LEASE_ID_FIELD, lease_id.as_str());

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
    active.lock().await.insert(lease_id.clone(), task_cancel.clone());

    let provision = PhaseTimer::start(SandboxPhase::Allocate);
    publish_lifecycle(js, &lifecycle_subj, &provision.phase_started()).await;

    let result = provider.lease(req_ref, lease_id).await;

    let out = match &result {
        Ok(ProviderLease { sandbox_id, .. }) => {
            publish_lifecycle(js, &lifecycle_subj, &provision.complete()).await;
            info!(lease_id = %lease_id, sandbox_id = %sandbox_id, tenant = %req_ref.tenant, "sandbox launched");
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
        Err(e) => {
            publish_lifecycle(js, &lifecycle_subj, &provision.fail(e.to_string())).await;
            warn!(lease_id = %lease_id, tenant = %req_ref.tenant, error = %e, "sandbox launch failed");
            publish_lifecycle(
                js,
                &lifecycle_subj,
                &LifecycleEvent::Failed {
                    sandbox_id: lease_sandbox_id,
                    error: e.to_string(),
                },
            )
            .await;
            result
        }
    };

    if out.is_err() {
        active.lock().await.remove(lease_id);
    }
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
    context: LeaseWorkerContext,
) -> anyhow::Result<()> {
    let LeaseWorkerContext {
        worker_id,
        current_leases,
        docker_provider,
    } = context;
    let reply = msg.reply.clone();

    if let Some(docker) = &docker_provider {
        let preview = serde_json::from_slice::<ProviderDispatchPayload>(&msg.payload);
        if let Ok(preview) = preview
            && let Err(error) = docker
                .require_legacy_intake(&preview.lease_id, preview.target.is_some())
                .await
                .map_err(ProviderError::backend)
        {
            if let Some(reply) = reply {
                client
                    .publish(
                        reply,
                        serde_json::to_vec(&trogon_sandbox_protocol::dispatch_failure_reply(&error))?.into(),
                    )
                    .await?;
            }
            return Ok(());
        }
    }
    let payload: ProviderDispatchPayload =
        match trogon_sandbox_protocol::decode_verified(&msg.payload, Some(&worker_id)) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "invalid dispatch payload");
                if let Some(ref subj) = reply
                    && let Ok(b) = serde_json::to_vec(&DecodeErrorReply {
                        decode_error: e.to_string(),
                    })
                {
                    let _ = client.publish(subj.clone(), b.into()).await;
                }
                return Ok(());
            }
        };

    if payload
        .target
        .as_ref()
        .is_some_and(|target| target.worker_id != worker_id)
    {
        if let Some(reply) = reply {
            let error = ProviderError::backend("dispatch targets a different Docker worker");
            client
                .publish(
                    reply,
                    serde_json::to_vec(&trogon_sandbox_protocol::dispatch_failure_reply(&error))?.into(),
                )
                .await?;
        }
        return Ok(());
    }

    info!(
        lease_id = %payload.lease_id,
        tenant = %payload.request.tenant,
        "lease accepted"
    );

    // A redelivered dispatch gets the first grant instead of a second container.
    current_leases.fetch_add(1, Ordering::Relaxed);
    let result = grants
        .get_or_launch(&payload.lease_id, || async {
            let mut grant = handle_lease_inner(&payload.request, &payload.lease_id, &js, &provider, &active).await?;
            grant.worker_id = Some(worker_id);
            Ok(grant)
        })
        .await;
    let result = match (result, docker_provider) {
        (Ok(grant), Some(docker)) => docker
            .bind_command_assignment(&payload)
            .await
            .map(|()| grant)
            .map_err(ProviderError::backend),
        (result, _) => result,
    };
    current_leases.fetch_sub(1, Ordering::Relaxed);

    if let Some(subj) = reply {
        let body = match &result {
            Ok(pl) => serde_json::to_vec(pl)?,
            Err(e) => serde_json::to_vec(&trogon_sandbox_protocol::dispatch_failure_reply(e))?,
        };
        client.publish(subj, body.into()).await?;
    }

    Ok(())
}

async fn handle_cancel_msg(
    msg: async_nats::Message,
    provider: Arc<dyn Provider>,
    docker_provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
    terminals: Arc<DockerTerminals>,
    client: Client,
) -> anyhow::Result<()> {
    let lease_id = extract_cancel_lease_id(msg.subject.as_str());

    if let Some(lease_id) = lease_id {
        docker_provider.require_legacy_intake(&lease_id, false).await?;
        trogon_sandbox_protocol::global_verifier().admit_message(
            msg.subject.as_str(),
            msg.headers.as_ref(),
            &msg.payload,
        )?;
        release_lease(&lease_id, &provider, &active, &grants, &terminals, &client).await;
    }

    Ok(())
}

/// Ends a lease on this worker: the path a broker cancel takes, shared with
/// the authority guard's purge so both leave the same state behind.
async fn release_lease(
    lease_id: &LeaseId,
    provider: &Arc<dyn Provider>,
    active: &ActiveLeases,
    grants: &RedispatchGuard,
    terminals: &DockerTerminals,
    client: &Client,
) {
    grants.forget(lease_id);
    terminals.close_lease(client, lease_id).await;
    if let Some(token) = active.lock().await.remove(lease_id) {
        token.cancel();
    }
    if let Err(e) = provider.cancel(lease_id).await {
        warn!(lease_id = %lease_id, error = %e, "cancel failed");
    } else {
        info!(lease_id = %lease_id, "lease released");
    }
}

/// What the authority guard needs from this worker.
struct DockerAuthorityHost {
    docker_provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
    terminals: Arc<DockerTerminals>,
    client: Client,
}

#[async_trait::async_trait]
impl trogon_sandbox_protocol::AuthorityHost for DockerAuthorityHost {
    async fn held_leases(&self) -> Result<Vec<trogon_sandbox_protocol::HeldLease>, AuthorityHostError> {
        self.docker_provider.authority_held_leases().await
    }

    async fn freeze(
        &self,
        lease: &trogon_sandbox_protocol::HeldLease,
    ) -> Result<trogon_sandbox_protocol::FreezeOutcome, AuthorityHostError> {
        self.docker_provider.authority_freeze(lease).await
    }

    async fn thaw(
        &self,
        lease: &trogon_sandbox_protocol::HeldLease,
    ) -> Result<trogon_sandbox_protocol::ThawOutcome, AuthorityHostError> {
        self.docker_provider.authority_thaw(lease).await
    }

    async fn purge(
        &self,
        lease: &trogon_sandbox_protocol::HeldLease,
    ) -> Result<trogon_sandbox_protocol::PurgeOutcome, AuthorityHostError> {
        self.docker_provider
            .authority_purge(lease, || async {
                self.terminals.close_lease(&self.client, lease.lease_id()).await;
                if let Some(token) = self.active.lock().await.remove(lease.lease_id()) {
                    token.cancel();
                }
                self.grants.forget(lease.lease_id());
            })
            .await
    }
}

async fn handle_snapshot_export_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: SnapshotExportRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            let outcome = SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
                message: format!("invalid snapshot export request: {e}"),
            });
            let body = serde_json::to_vec(&outcome)?;
            client.publish(reply, Bytes::from(body)).await?;
            return Ok(());
        }
    };

    let outcome = match provider.export_for_lease(&request.lease_id, &request.target).await {
        Ok(receipt) => SnapshotExportOutcome::Ok(receipt),
        Err(e) => SnapshotExportOutcome::Err(SnapshotExportError::Transient { message: e.to_string() }),
    };

    let body = serde_json::to_vec(&outcome)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast checkpoint_running request. Unlike snapshot export
/// (which is worker_id-scoped and thus unambiguous), this subject is
/// broadcast to every docker worker, so only the worker actually holding the
/// lease in its local `active` map acts and replies.
async fn handle_checkpoint_running_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: CheckpointRunningRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid checkpoint_running request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let outcome = match provider.checkpoint_for_lease(&request.lease_id, &request.target).await {
        Ok(receipt) => SnapshotExportOutcome::Ok(receipt),
        Err(e) => SnapshotExportOutcome::Err(SnapshotExportError::Transient { message: e.to_string() }),
    };

    let body = serde_json::to_vec(&outcome)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast network_policy_update request. Like checkpoint_running,
/// this subject is broadcast to every docker worker, so only the worker
/// actually holding the lease in its local `active` map acts and replies.
async fn handle_network_policy_update_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: NetworkPolicyUpdateRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid network_policy_update request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = match provider
        .push_network_policy_for_lease(&request.lease_id, &request.network_policy)
        .await
    {
        Ok(()) => NetworkPolicyUpdateResponse::Applied,
        Err(e) => NetworkPolicyUpdateResponse::Denied(LeaseError::Backend { message: e.to_string() }),
    };

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

async fn handle_run_command_verified(
    verified: VerifiedRequest,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let msg = verified.message;
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };
    let payload = trogon_sandbox_protocol::nats_transfer::receive(&client, &msg).await?;
    let request: RunCommandRequest = match serde_json::from_slice(&payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid run_command request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let max_output_bytes = trogon_sandbox_protocol::effective_max_output_bytes(request.max_output_bytes, usize::MAX);
    let redaction = trogon_sandbox_protocol::RedactionSet::from_request_values(&request.redact_values);
    let response = if request.require_launch_cancellation.is_required()
        || (verified.scope.is_some() && provider.supervises_closed_command(&request))
    {
        match verified.scope.as_ref() {
            Some(scope) => {
                provider
                    .run_controlled_command(&request, scope, msg.subject.as_str(), &payload)
                    .await
            }
            None => RunCommandResponse::Denied(LeaseError::InvalidSpec {
                reason: "command launch cancellation requires exact verified command authority".into(),
            }),
        }
    } else {
        match request.execution {
            CommandExecution::Detached {
                stdin,
                deadline,
                process,
            } => {
                match provider
                    .run_command_for_lease_detached_redacting_with_origin(
                        &request.lease_id,
                        request.command,
                        request.cwd,
                        &request.env,
                        request.timeout_seconds,
                        request.run_as_user.as_ref(),
                        request.stream_output,
                        stdin,
                        request.output_retention,
                        deadline,
                        max_output_bytes,
                        process,
                        redaction.clone(),
                    )
                    .await
                {
                    Ok(started) => RunCommandResponse::Started(started),
                    Err(e) => RunCommandResponse::Denied(LeaseError::Backend {
                        message: redaction.redact_str(&e.to_string()),
                    }),
                }
            }
            CommandExecution::Attached => {
                match provider
                    .run_command_for_lease_redacting(
                        &request.lease_id,
                        request.command,
                        request.cwd,
                        &request.env,
                        request.timeout_seconds,
                        request.run_as_user.as_ref(),
                        request.stream_output,
                        max_output_bytes,
                        &redaction,
                    )
                    .await
                {
                    Ok(output) => RunCommandResponse::Completed(output),
                    Err(e) => RunCommandResponse::Denied(LeaseError::Backend {
                        message: redaction.redact_str(&e.to_string()),
                    }),
                }
            }
        }
    };

    let body = encode_run_within(response, client.server_info().max_payload)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

async fn handle_cancel_command_launch(
    verified: VerifiedRequest,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let message = verified.message;
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = trogon_sandbox_protocol::nats_transfer::receive(&client, &message).await?;
    let request: CancelCommandLaunchRequest = serde_json::from_slice(&payload)?;
    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }
    let response = match verified.scope.as_ref() {
        Some(scope) => {
            provider
                .cancel_controlled_command(&request, scope, message.subject.as_str(), &payload)
                .await
        }
        None => CancelCommandLaunchResponse::Unsupported,
    };
    client.publish(reply, serde_json::to_vec(&response)?.into()).await?;
    Ok(())
}

/// Handles a broadcast poll_command request. Routed like
/// `run_command`: only the worker actually holding `lease_id`
/// (and thus potentially tracking `execution_id` from a prior detached
/// `run_command`) acts and replies.
async fn handle_poll_command_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: PollCommandRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid poll_command request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = match provider
        .poll_command_for_lease(&request.lease_id, &request.execution_id)
        .await
    {
        Some(status) => status,
        None => PollCommandResponse::NotFound,
    };

    let body = encode_poll_within(response, client.server_info().max_payload)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast list_processes request. Only the worker holding
/// `lease_id` replies.
async fn handle_list_processes_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: ListProcessesRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid list_processes request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = provider.list_processes_for_lease(&request.lease_id).await;
    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast wait_ready request. Only the worker holding
/// `lease_id` replies; the wait is bounded by the request's timeout.
async fn handle_wait_ready_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: WaitReadyRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid wait_ready request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let timeout = std::time::Duration::from_secs(u64::from(
        request.timeout_seconds.min(WaitReadyRequest::MAX_TIMEOUT_SECONDS),
    ));
    let response = provider
        .wait_ready_for_lease(&request.lease_id, &request.execution_id, timeout)
        .await;
    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast kill_command request. Routed like
/// [`handle_poll_command_msg`]: only the worker actually holding `lease_id`
/// (and thus potentially tracking `execution_id` from a prior detached
/// `run_command`) acts and replies.
async fn handle_kill_command_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: KillCommandRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid kill_command request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = provider
        .kill_command_for_lease(&request.lease_id, &request.execution_id, request.signal)
        .await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast stop_command request. Routed like
/// [`handle_kill_command_msg`].
async fn handle_stop_command_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: StopCommandRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid stop_command request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let grace = std::time::Duration::from_secs(u64::from(
        request.grace_seconds.min(StopCommandRequest::MAX_GRACE_SECONDS),
    ));
    let response = provider
        .stop_command_for_lease(&request.lease_id, &request.execution_id, grace)
        .await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast restart_command request. Routed like
/// [`handle_kill_command_msg`].
async fn handle_restart_command_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: RestartCommandRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid restart_command request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let grace = std::time::Duration::from_secs(u64::from(
        request.grace_seconds.min(StopCommandRequest::MAX_GRACE_SECONDS),
    ));
    let response = provider
        .restart_command_for_lease(&request.lease_id, &request.execution_id, grace)
        .await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast write_stdin request. Routed like
/// [`handle_kill_command_msg`]: only the worker actually holding `lease_id`
/// acts and replies.
async fn handle_write_stdin_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: WriteStdinRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid write_stdin request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(request.lease_id()) {
        return Ok(());
    }

    let response = provider
        .write_stdin_for_lease(
            request.lease_id(),
            request.execution_id(),
            request.seq(),
            request.data().clone(),
            request.eof(),
        )
        .await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// What the SSH stream bridge needs from the docker worker: the leases it
/// holds and its terminals.
struct DockerSshHost {
    provider: Arc<DockerWorkerProvider>,
    client: Client,
    terminals: Arc<DockerTerminals>,
    active: ActiveLeases,
}

#[async_trait::async_trait]
impl trogon_sandbox_ssh_gateway::SshTerminalHost for DockerSshHost {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.active.lock().await.contains_key(lease_id)
    }

    async fn terminal(
        &self,
        req: &trogon_sandbox_contract::TerminalRequest,
    ) -> trogon_sandbox_contract::TerminalResponse {
        match serde_json::to_vec(req) {
            Ok(body) => self.terminals.handle(&self.provider, &self.client, &body).await,
            Err(e) => trogon_sandbox_contract::TerminalResponse::Failed(
                trogon_sandbox_contract::TerminalError::InvalidRequest { reason: e.to_string() },
            ),
        }
    }
}

/// Handles a broadcast SSH attach request (`sandbox.ssh.attach.>`). Routed
/// like `run_command`: every docker worker subscribes to the
/// wildcard subject, but only the worker actually holding `lease_id` acts and
/// replies.
///
/// The session is served over the stream bridge onto the lease's terminals,
/// which needs no sshd. Installing the session key for an in-container sshd
/// stays as a best effort for images that ship one.
async fn handle_ssh_attach_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    bridge: Arc<trogon_sandbox_ssh_gateway::SshStreamBridge>,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: SshAttachRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid ssh attach request");
            return Ok(());
        }
    };

    let Some(response) = bridge.attach(&request).await else {
        return Ok(());
    };
    if response.ok
        && let Err(e) = provider
            .ssh_attach_for_lease(&request.lease_id, request.session_id.as_str(), &request.public_key)
            .await
    {
        warn!(error = %e, "ssh session key was not installed; the stream bridge still serves the session");
    }

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast SSH detach request (`sandbox.ssh.detach.>`). Routed
/// like [`handle_ssh_attach_msg`].
async fn handle_ssh_detach_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    bridge: Arc<trogon_sandbox_ssh_gateway::SshStreamBridge>,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: SshDetachRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid ssh detach request");
            return Ok(());
        }
    };

    let Some(response) = bridge.detach(&request).await else {
        return Ok(());
    };
    if response.ok
        && let Err(e) = provider
            .ssh_detach_for_lease(&request.lease_id, request.session_id.as_str())
            .await
    {
        warn!(error = %e, "ssh session key was not removed");
    }

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast get_domain request. Routed like
/// `run_command`: every docker worker subscribes to the
/// wildcard subject, but only the worker actually holding `lease_id` acts and
/// replies.
async fn handle_get_domain_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
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

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = provider
        .get_sandbox_domain_for_lease(&request.lease_id, request.port)
        .await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast native_handle request. Routed like
/// [`handle_get_domain_msg`]: only the worker holding `lease_id` replies.
async fn handle_native_handle_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: trogon_sandbox_contract::NativeHandleRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid native_handle request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = provider.native_handle_for_lease(&request.lease_id).await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast pause or resume request. Routed like
/// [`handle_get_domain_msg`]: only the worker holding `lease_id` replies.
async fn handle_freeze_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
    pause: bool,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: SandboxFreezeRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid pause/resume request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = provider.set_paused_for_lease(&request.lease_id, pause).await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Handles a broadcast resource usage request. Routed like
/// [`handle_freeze_msg`]: only the worker holding `lease_id` replies.
async fn handle_resource_usage_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<DockerWorkerProvider>,
    active: ActiveLeases,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };

    let request: ResourceUsageRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid resource usage request");
            return Ok(());
        }
    };

    if !active.lock().await.contains_key(&request.lease_id) {
        return Ok(());
    }

    let response = provider.resource_usage_for_lease(&request.lease_id).await;

    let body = serde_json::to_vec(&response)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

pub(crate) fn extract_cancel_lease_id(subject: &str) -> Option<LeaseId> {
    subject
        .split('.')
        .next_back()
        .filter(|s| !s.is_empty())
        .map(LeaseId::new)
}

#[cfg(test)]
pub(crate) async fn process_lease_request(
    req: trogon_sandbox_contract::LeaseRequest,
    provider: Arc<dyn Provider>,
) -> Result<ProviderLease, ProviderError> {
    let lease_id = LeaseId::new(uuid::Uuid::now_v7().to_string());
    provider.lease(&req, &lease_id).await
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

mod rollback;
use rollback::handle_rollback_msg;
