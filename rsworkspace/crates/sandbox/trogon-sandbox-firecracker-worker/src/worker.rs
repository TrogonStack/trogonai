//! The worker's NATS surface.
//!
//! Two shapes of subject arrive here. `lease` is a queue of work any worker may take, and
//! everything else is a broadcast addressed to whichever worker happens to hold the lease.
//! The broadcast handlers all begin the same way: ask the provider for the lease, and stay
//! silent when it is not ours, because a reply from a worker that does not hold the lease
//! would race the one from the worker that does.
//!
//! Subjects this provider cannot serve are subscribed anyway and answered with a refusal.
//! Leaving them unsubscribed would make the broker wait out its timeout and report a generic
//! backend failure, which reads to a tenant as an outage rather than as an unsupported
//! feature.

mod constants;
pub use constants::HEARTBEAT_INTERVAL;
mod replies;

use std::sync::{Arc, Mutex, PoisonError};

use async_nats::Client;
use async_nats::jetstream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    CapabilitySupport, CommandDeadline, CommandExecution, CommandOutputChunk, CreateSandboxGroupRequest,
    CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse, ExecutionId, KillCommandRequest,
    KillCommandResponse, LeaseError, LeaseId, LeaseReleasedReason, LifecycleEvent, ModifyGroupMembershipRequest,
    ModifyGroupMembershipResponse, NetworkPolicyUpdateRequest, PollCommandRequest, PollCommandResponse,
    ProviderCapabilityProfile, ProviderDispatchPayload, ProviderKind, ReadFileRequest, ReadFileResponse,
    RunCommandRequest, RunCommandResponse, SandboxId, SnapshotExportError, SnapshotExportOutcome,
    SnapshotExportRequest, WorkerCapabilities, WorkerId, WriteFilesRequest, WriteFilesResponse, WriteStdinRequest,
    WriteStdinResponse,
};
use trogon_sandbox_protocol::{
    AuthorityHostError, DETACHED_EXIT_POLL_FAILURES, DETACHED_EXIT_POLL_INTERVAL, await_detached_exit,
    encode_read_file_response_within, fit_poll_command_response, fit_run_command_response, lifecycle_subject,
    publish_command_exit, spawn_heartbeat_loop, subjects, subscribe_verified,
};
use trogon_std::env::ReadEnv;

use crate::adapter::{FirecrackerProvider, LeaseAccess, LeaseLoss, LeaseLossSink};
use crate::agent::{ChunkSink, DiscardChunks, GuestAgent};

/// The wildcard form of a per-lease provider subject.
fn verb_subject(verb: &str) -> String {
    format!(
        "sandbox.provider.{}.{verb}.>",
        subjects::provider_token(ProviderKind::Firecracker)
    )
}

/// Where live output for one command is published while it runs.
fn chunk_subject(lease_id: &LeaseId, execution_id: &ExecutionId) -> String {
    format!(
        "sandbox.evidence.{}.{}",
        lease_id.as_str(),
        subjects::evidence_command_suffix(execution_id)
    )
}

/// Forwards the agent's output frames onto the lease's evidence stream.
///
/// The execution id is minted by the agent, so the subject is derived per chunk rather than
/// fixed when the sink is built.
struct NatsChunkSink {
    js: jetstream::Context,
    lease_id: LeaseId,
    /// The agent's execution id, learned from the first chunk.
    execution_id: Mutex<Option<ExecutionId>>,
}

impl NatsChunkSink {
    fn new(js: jetstream::Context, lease_id: LeaseId) -> Self {
        Self {
            js,
            lease_id,
            execution_id: Mutex::new(None),
        }
    }

    fn known_execution(&self) -> Option<ExecutionId> {
        self.execution_id.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Announces that the streamed command exited, after its last chunk, so
    /// log followers end at once instead of polling for the exit. The agent
    /// names the execution in a begin frame even when the command prints
    /// nothing; only an agent that predates that frame leaves a silent
    /// command unnamed, and then there is nothing to announce.
    async fn publish_exit(&self, exit_code: Option<i32>) {
        let Some(execution_id) = self.known_execution() else {
            return;
        };
        publish_exit_marker(&self.js, &self.lease_id, &execution_id, exit_code).await;
    }
}

async fn publish_exit_marker(
    js: &jetstream::Context,
    lease_id: &LeaseId,
    execution_id: &ExecutionId,
    exit_code: Option<i32>,
) {
    if let Err(e) = publish_command_exit(js, lease_id, execution_id, exit_code).await {
        warn!(lease_id = %lease_id, execution_id = %execution_id, error = %e, "failed to publish command exit marker");
    }
}

/// Watches a detached streamed command until it exits and then announces the
/// exit, since its `Started` reply leaves nothing on the request path to see it.
fn watch_detached_exit(agent: GuestAgent, js: jetstream::Context, lease_id: LeaseId, execution_id: ExecutionId) {
    tokio::spawn(async move {
        let request = PollCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
        };
        let exit = await_detached_exit(
            || agent.poll(&request),
            DETACHED_EXIT_POLL_INTERVAL,
            DETACHED_EXIT_POLL_FAILURES,
        )
        .await;
        publish_exit_marker(&js, &lease_id, &execution_id, exit.exit_code()).await;
    });
}

#[async_trait]
impl ChunkSink for NatsChunkSink {
    async fn begin(&self, execution_id: ExecutionId) {
        self.execution_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(execution_id);
    }

    async fn publish(&self, chunk: CommandOutputChunk) {
        self.execution_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(|| chunk.execution_id.clone());
        let subject = chunk_subject(&self.lease_id, &chunk.execution_id);
        match serde_json::to_vec(&chunk) {
            Ok(payload) => {
                if let Err(e) = self.js.publish(subject.clone(), payload.into()).await {
                    warn!(subject, error = %e, "failed to publish command output chunk");
                }
            }
            Err(e) => warn!(error = %e, "failed to serialize command output chunk"),
        }
    }
}

/// Reports a lease whose VM is gone the way a failed launch is reported, then asks the
/// broker to release it as `Failed`, since the broker only learns a lease is over through a
/// cancel or its TTL and would otherwise keep routing verbs and holding quota for it.
pub struct NatsLeaseLoss {
    client: Client,
    js: jetstream::Context,
}

impl NatsLeaseLoss {
    pub fn new(client: Client, js: jetstream::Context) -> Self {
        Self { client, js }
    }
}

#[async_trait]
impl LeaseLossSink for NatsLeaseLoss {
    async fn lost(&self, lease_id: &LeaseId, loss: &LeaseLoss) {
        publish_lease_event(&self.js, lease_id, |sandbox_id| LifecycleEvent::Failed {
            sandbox_id,
            error: loss.to_string(),
        })
        .await;
        let reason = serde_json::to_vec(&LeaseReleasedReason::Failed).unwrap_or_default();
        if let Err(e) = self
            .client
            .publish(subjects::lease_cancel(lease_id), Bytes::from(reason))
            .await
        {
            warn!(lease_id = %lease_id, error = %e, "asking the broker to release a lost lease failed");
        }
    }
}

pub async fn run(
    client: Client,
    js: jetstream::Context,
    provider: Arc<FirecrackerProvider>,
    worker_id: WorkerId,
    cancel: CancellationToken,
    env: &impl ReadEnv,
) -> anyhow::Result<()> {
    let authority =
        trogon_sandbox_protocol::AuthorityConfig::new(trogon_sandbox_protocol::AuthorityMode::from_env(env)?);
    let replay_bytes = trogon_sandbox_protocol::terminal_replay_bytes_from_env(env);
    serve(client, js, provider, worker_id, cancel, authority, replay_bytes).await
}

pub async fn run_with_authority(
    client: Client,
    js: jetstream::Context,
    provider: Arc<FirecrackerProvider>,
    worker_id: WorkerId,
    cancel: CancellationToken,
    authority: trogon_sandbox_protocol::AuthorityConfig,
) -> anyhow::Result<()> {
    serve(
        client,
        js,
        provider,
        worker_id,
        cancel,
        authority,
        trogon_sandbox_protocol::TerminalReplayRing::DEFAULT_CAPACITY,
    )
    .await
}

async fn serve(
    client: Client,
    js: jetstream::Context,
    provider: Arc<FirecrackerProvider>,
    worker_id: WorkerId,
    cancel: CancellationToken,
    authority: trogon_sandbox_protocol::AuthorityConfig,
    terminal_replay_bytes: usize,
) -> anyhow::Result<()> {
    let ledger = provider.dispatch_ledger().await?;
    anyhow::ensure!(
        &worker_id == ledger.worker_id(),
        "worker ID differs from its durable dispatch ledger"
    );
    let mut lease_sub = subscribe_verified(
        &client,
        subjects::worker_dispatch(ProviderKind::Firecracker, ledger.worker_id()),
    )
    .await?;
    let mut cleanup_sub = subscribe_verified(
        &client,
        subjects::worker_cleanup(ProviderKind::Firecracker, ledger.worker_id()),
    )
    .await?;
    let mut run_command_sub = subscribe_verified(&client, verb_subject("run_command")).await?;
    let mut poll_command_sub = subscribe_verified(&client, verb_subject("poll_command")).await?;
    let mut kill_command_sub = subscribe_verified(&client, verb_subject("kill_command")).await?;
    let mut write_stdin_sub = subscribe_verified(&client, verb_subject("write_stdin")).await?;
    let mut write_files_sub = subscribe_verified(&client, verb_subject("write_files")).await?;
    let mut read_file_sub = subscribe_verified(&client, verb_subject("read_file")).await?;
    let mut file_op_sub = subscribe_verified(&client, verb_subject("file_op")).await?;
    let mut create_user_sub = subscribe_verified(&client, verb_subject("create_user")).await?;
    let mut create_group_sub = subscribe_verified(&client, verb_subject("create_group")).await?;
    let mut modify_group_membership_sub = subscribe_verified(&client, verb_subject("modify_group_membership")).await?;
    let mut checkpoint_running_sub = subscribe_verified(&client, verb_subject("checkpoint_running")).await?;
    let mut network_policy_update_sub = subscribe_verified(&client, verb_subject("network_policy_update")).await?;
    let mut snapshot_sub = subscribe_verified(&client, subjects::worker_snapshot_export(&worker_id)).await?;
    let mut terminal_sub = subscribe_verified(&client, verb_subject("terminal")).await?;
    let terminals =
        crate::terminal::FirecrackerTerminals::with_replay_bytes(Arc::new(client.clone()), terminal_replay_bytes);
    client.flush().await?;
    spawn_heartbeat_task(client.clone(), Arc::clone(&provider), worker_id.clone(), cancel.clone());
    let mut tasks = tokio::task::JoinSet::new();
    let host = trogon_sandbox_protocol::LedgerAuthorityHost::new(ledger.clone(), {
        let provider = provider.clone();
        move |request: trogon_sandbox_contract::ProviderCleanupRequest| {
            let provider = provider.clone();
            async move { Ok::<_, AuthorityHostError>(provider.cleanup_dispatch(&request).await) }
        }
    });
    let guard = trogon_sandbox_protocol::AuthorityGuard::new(
        authority,
        ledger.worker_id().clone(),
        Arc::new(trogon_sandbox_protocol::NatsAuthorityOracle::new(
            client.clone(),
            ledger.worker_id().clone(),
        )),
        Arc::new(host),
    );
    tasks.spawn({
        let cancel = cancel.clone();
        async move { guard.run(cancel).await }
    });
    info!(worker_id = %worker_id, "worker ready");

    loop {
        tokio::select! {
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result { warn!(%error, "worker request task failed"); }
            }
            _ = cancel.cancelled() => {
                info!("worker shutting down");
                break;
            }
            maybe_msg = lease_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let js = js.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_lease_msg(msg, client, js, provider).await {
                        warn!(error = %e, "failed to handle lease request");
                    }
                });
            }
            Some(msg) = cleanup_sub.next() => {
                let provider = Arc::clone(&provider);
                let client = client.clone();
                tasks.spawn(async move {
                    let result = async {
                        let request: trogon_sandbox_contract::ProviderCleanupRequest = serde_json::from_slice(&msg.payload)?;
                        let response = provider.cleanup_dispatch(&request).await;
                        if let Some(reply) = msg.reply { client.publish(reply, serde_json::to_vec(&response)?.into()).await?; }
                        anyhow::Ok(())
                    }.await;
                    if let Err(error) = result { warn!(%error, "targeted cleanup request failed"); }
                });
            }
            maybe_msg = run_command_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let js = js.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_run_command_msg(msg, client, js, provider).await {
                        warn!(error = %e, "failed to handle run_command request");
                    }
                });
            }
            maybe_msg = poll_command_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_poll_command_msg(msg, client, provider).await {
                        warn!(error = %e, "failed to handle poll_command request");
                    }
                });
            }
            maybe_msg = kill_command_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_kill_command_msg(msg, client, provider).await {
                        warn!(error = %e, "failed to handle kill_command request");
                    }
                });
            }
            maybe_msg = write_stdin_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_write_stdin_msg(msg, client, provider).await {
                        warn!(error = %e, "failed to handle write_stdin request");
                    }
                });
            }
            maybe_msg = terminal_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                let terminals = Arc::clone(&terminals);
                tasks.spawn(async move {
                    if let Err(e) = handle_terminal_msg(msg, client, provider, terminals).await {
                        warn!(error = %e, "failed to handle terminal request");
                    }
                });
            }
            maybe_msg = write_files_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_write_files_msg(msg, client, provider).await {
                        warn!(error = %e, "failed to handle write_files request");
                    }
                });
            }
            maybe_msg = read_file_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_read_file_msg(msg, client, provider).await {
                        warn!(error = %e, "failed to handle read_file request");
                    }
                });
            }
            maybe_msg = file_op_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_file_op_msg(msg, client, provider).await {
                        warn!(error = %e, "failed to handle file_op request");
                    }
                });
            }
            maybe_msg = create_user_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_create_user_msg(msg, client, provider).await {
                        warn!(error = %e, "failed to handle create_user request");
                    }
                });
            }
            maybe_msg = create_group_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_create_group_msg(msg, client, provider).await {
                        warn!(error = %e, "failed to handle create_group request");
                    }
                });
            }
            maybe_msg = modify_group_membership_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_modify_group_membership_msg(msg, client, provider).await
                    {
                        warn!(error = %e, "failed to handle modify_group_membership request");
                    }
                });
            }
            maybe_msg = checkpoint_running_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_snapshot_msg(msg, client, provider, true).await
                    {
                        warn!(error = %e, "failed to handle checkpoint_running request");
                    }
                });
            }
            maybe_msg = network_policy_update_sub.next() => {
                let Some(msg)=maybe_msg else {break};let client=client.clone();let provider=Arc::clone(&provider);
                tasks.spawn(async move{if let Err(error)=handle_network_policy_update_msg(msg,client,provider).await{warn!(%error,"network policy update failed");}});
            }
            maybe_msg = snapshot_sub.next() => {
                let Some(msg) = maybe_msg else { break };
                let client = client.clone();
                let provider = Arc::clone(&provider);
                tasks.spawn(async move {
                    if let Err(e) = handle_snapshot_msg(msg, client, provider, false).await {
                        warn!(error = %e, "failed to handle snapshot export");
                    }
                });
            }
        }
    }

    cancel.cancel();
    if tokio::time::timeout(std::time::Duration::from_secs(300), async {
        while tasks.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    Ok(())
}

/// Publishes capacity until `cancel` fires.
///
/// `current_leases` comes from the provider's own map, which recovery has already
/// repopulated, so a restarted worker does not briefly advertise itself as empty and attract
/// every pending lease.
pub fn spawn_heartbeat_task(
    client: Client,
    provider: Arc<FirecrackerProvider>,
    worker_id: WorkerId,
    cancel: CancellationToken,
) {
    spawn_heartbeat_loop(
        client,
        ProviderKind::Firecracker,
        worker_id.clone(),
        HEARTBEAT_INTERVAL,
        cancel,
        move || {
            let (worker_id, provider) = (worker_id.clone(), provider.clone());
            async move {
                provider.sweep_warm_pool().await;
                let mut profile = provider.profile();
                let warm = &provider.config().warm_pool;
                let warm_enabled = warm.max_snapshots > 0 && warm.max_bytes > 0;
                profile.lifecycle.supports_warm_pool = CapabilitySupport::from(warm_enabled);
                profile.lifecycle.max_warm_pool_size =
                    warm_enabled.then(|| u32::try_from(warm.max_snapshots).unwrap_or(u32::MAX));
                build_heartbeat(
                    &worker_id,
                    provider.config().max_concurrent_leases,
                    provider.current_lease_count().await,
                    profile,
                    provider.cached_digests().await,
                )
            }
        },
    );
}

pub(crate) fn build_heartbeat(
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
    profile: ProviderCapabilityProfile,
    cached_digests: Vec<trogon_sandbox_contract::Digest>,
) -> trogon_sandbox_contract::WorkerHeartbeat {
    trogon_sandbox_contract::WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::Firecracker,
        capacity: trogon_sandbox_contract::WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: std::time::SystemTime::now(),
        cached_digests,
        profile,
        warm_pool_population: std::collections::HashMap::new(),
        dispatch_protocol: trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1,
        native_volume_bindings: Default::default(),
    }
}

async fn handle_lease_msg(
    msg: async_nats::Message,
    client: Client,
    js: jetstream::Context,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let reply = msg.reply.clone();

    let payload: ProviderDispatchPayload = match trogon_sandbox_protocol::decode_verified(&msg.payload, None) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "invalid dispatch payload");
            if let Some(subject) = reply
                && let Ok(body) = serde_json::to_vec(&replies::DecodeErrorReply {
                    decode_error: &e.to_string(),
                })
            {
                let _ = client.publish(subject, body.into()).await;
            }
            return Ok(());
        }
    };

    info!(
        lease_id = %payload.lease_id,
        tenant = %payload.request.tenant,
        "lease accepted"
    );

    publish_lease_event(&js, &payload.lease_id, |sandbox_id| LifecycleEvent::Started {
        sandbox_id,
    })
    .await;

    // Every phase in between is emitted by the provider itself, which is the only thing that
    // knows which of them a given lease actually goes through.
    let result = provider.lease_dispatch(&payload).await;

    match &result {
        Ok(lease) => {
            info!(
                lease_id = %payload.lease_id,
                sandbox_id = %lease.sandbox_id,
                tenant = %payload.request.tenant,
                "sandbox launched"
            );
            publish_lifecycle(
                &js,
                &lifecycle_subject(&payload.lease_id),
                &LifecycleEvent::Completed {
                    sandbox_id: lease.sandbox_id.clone(),
                },
            )
            .await;
        }
        Err(e) => {
            warn!(
                lease_id = %payload.lease_id,
                tenant = %payload.request.tenant,
                error = %e,
                "sandbox launch failed"
            );
            publish_lease_event(&js, &payload.lease_id, |sandbox_id| LifecycleEvent::Failed {
                sandbox_id,
                error: e.to_string(),
            })
            .await;
        }
    }

    if let Some(subject) = reply {
        let body = match &result {
            Ok(lease) => serde_json::to_vec(lease)?,
            Err(e) => serde_json::to_vec(&replies::ProviderErrorReply {
                provider_error: &e.to_string(),
            })?,
        };
        client.publish(subject, body.into()).await?;
    }

    Ok(())
}

async fn handle_run_command_msg(
    msg: async_nats::Message,
    client: Client,
    js: jetstream::Context,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<RunCommandRequest>(&msg, "run_command") else {
        return Ok(());
    };

    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        &request.lease_id,
        RunCommandResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };

    if let Some(reason) = run_command_spec_violation(&request) {
        let response = RunCommandResponse::Denied(LeaseError::InvalidSpec { reason });
        return respond(&client, reply, &response).await;
    }

    let response = exec_streamed(&agent, js, &request).await;

    let response = fit_run_command_response(response, client.server_info().max_payload);
    respond(&client, reply, &response).await
}

/// Runs a command on the agent, publishing its output and exit marker on the evidence
/// stream when it asked to stream.
async fn exec_streamed(agent: &GuestAgent, js: jetstream::Context, request: &RunCommandRequest) -> RunCommandResponse {
    let streaming = request
        .stream_output
        .is_enabled()
        .then(|| NatsChunkSink::new(js, request.lease_id.clone()));
    let sink: &dyn ChunkSink = match &streaming {
        Some(sink) => sink,
        None => &DiscardChunks,
    };

    let response = agent
        .exec(request, sink)
        .await
        .unwrap_or_else(|e| RunCommandResponse::Denied(backend(&e)));
    if let Some(sink) = &streaming {
        match &response {
            RunCommandResponse::Completed(output) => sink.publish_exit(Some(output.exit_code)).await,
            RunCommandResponse::Started(started) => watch_detached_exit(
                agent.clone(),
                sink.js.clone(),
                request.lease_id.clone(),
                started.execution_id.clone(),
            ),
            // The stream broke after the command was named: its exit is unknown, but a
            // follower waiting on it must still be released.
            RunCommandResponse::Denied(_) => sink.publish_exit(None).await,
        }
    }
    response
}

async fn handle_poll_command_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<PollCommandRequest>(&msg, "poll_command") else {
        return Ok(());
    };

    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        &request.lease_id,
        PollCommandResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };

    let response = agent
        .poll(&request)
        .await
        .unwrap_or_else(|e| PollCommandResponse::Denied(backend(&e)));

    let response = fit_poll_command_response(response, client.server_info().max_payload);
    respond(&client, reply, &response).await
}

async fn handle_kill_command_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<KillCommandRequest>(&msg, "kill_command") else {
        return Ok(());
    };

    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        &request.lease_id,
        KillCommandResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };

    let response = agent
        .kill(&request)
        .await
        .unwrap_or_else(|e| KillCommandResponse::Denied(backend(&e)));

    respond(&client, reply, &response).await
}

async fn handle_write_stdin_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<WriteStdinRequest>(&msg, "write_stdin") else {
        return Ok(());
    };

    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        request.lease_id(),
        WriteStdinResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };

    let response = agent
        .write_stdin(&request)
        .await
        .unwrap_or_else(|e| WriteStdinResponse::Denied(backend(&e)));

    respond(&client, reply, &response).await
}

/// Handles a broadcast terminal request. Routed like [`handle_write_stdin_msg`]: only the
/// worker holding the lease acts and replies.
async fn handle_terminal_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
    terminals: Arc<crate::terminal::FirecrackerTerminals>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<trogon_sandbox_contract::TerminalRequest>(&msg, "terminal") else {
        return Ok(());
    };
    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        &request.lease_id,
        trogon_sandbox_contract::TerminalResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };
    let response = terminals.handle(&agent, &request).await;
    respond(&client, reply, &response).await
}

fn run_command_spec_violation(request: &RunCommandRequest) -> Option<String> {
    if request.require_launch_cancellation.is_required() {
        return Some("launch cancellation is not supported by this provider".to_string());
    }
    if let CommandExecution::Detached {
        deadline: CommandDeadline::Lease,
        ..
    } = &request.execution
    {
        return Some("lease deadline is not supported by this provider".to_string());
    }
    None
}

async fn handle_write_files_msg(
    mut msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    msg.payload = trogon_sandbox_protocol::nats_transfer::receive(&client, &msg).await?;
    let Some((reply, request)) = accept::<WriteFilesRequest>(&msg, "write_files") else {
        return Ok(());
    };

    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        &request.lease_id,
        WriteFilesResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };

    let response = agent
        .write_files(&request)
        .await
        .unwrap_or_else(|e| WriteFilesResponse::Denied(backend(&e)));

    trogon_sandbox_protocol::nats_transfer::respond(&client, reply.as_str(), serde_json::to_vec(&response)?.into())
        .await?;
    Ok(())
}

async fn handle_read_file_msg(
    mut msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    msg.payload = trogon_sandbox_protocol::nats_transfer::receive(&client, &msg).await?;
    let Some((reply, request)) = accept::<ReadFileRequest>(&msg, "read_file") else {
        return Ok(());
    };

    let Some(agent) = reach(&client, &reply, &provider, &request.lease_id, ReadFileResponse::Denied).await? else {
        return Ok(());
    };

    let response = agent
        .read_file(&request)
        .await
        .unwrap_or_else(|e| ReadFileResponse::Denied(backend(&e)));

    let body = encode_read_file_response_within(&response, &request.path, client.server_info().max_payload)?;
    trogon_sandbox_protocol::nats_transfer::respond(&client, reply.as_str(), body.into()).await?;
    Ok(())
}

/// Answers a `file_op` request for a lease this worker holds with a typed `Unsupported`
/// reply, so the caller learns the provider cannot serve it instead of timing out.
async fn handle_file_op_msg(
    mut msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };
    msg.payload = trogon_sandbox_protocol::nats_transfer::receive(&client, &msg).await?;
    let request: trogon_sandbox_contract::FileOpRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "invalid file_op request");
            return Ok(());
        }
    };
    if matches!(
        provider.access(&request.lease_id).await,
        crate::adapter::LeaseAccess::NotHeld
    ) {
        return Ok(());
    }
    let response = trogon_sandbox_protocol::unsupported_file_op_response("firecracker", &request.op);
    let body = trogon_sandbox_protocol::encode_file_op_response_within(&response, client.server_info().max_payload)?;
    trogon_sandbox_protocol::nats_transfer::respond(&client, reply.as_str(), body.into()).await?;
    Ok(())
}

async fn handle_create_user_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<CreateSandboxUserRequest>(&msg, "create_user") else {
        return Ok(());
    };

    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        &request.lease_id,
        CreateSandboxUserResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };

    let response = agent
        .create_user(&request)
        .await
        .unwrap_or_else(|e| CreateSandboxUserResponse::Denied(backend(&e)));

    respond(&client, reply, &response).await
}

async fn handle_create_group_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<CreateSandboxGroupRequest>(&msg, "create_group") else {
        return Ok(());
    };

    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        &request.lease_id,
        CreateSandboxGroupResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };

    let response = agent
        .create_group(&request)
        .await
        .unwrap_or_else(|e| CreateSandboxGroupResponse::Denied(backend(&e)));

    respond(&client, reply, &response).await
}

async fn handle_modify_group_membership_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<ModifyGroupMembershipRequest>(&msg, "modify_group_membership") else {
        return Ok(());
    };

    let Some(agent) = reach(
        &client,
        &reply,
        &provider,
        &request.lease_id,
        ModifyGroupMembershipResponse::Denied,
    )
    .await?
    else {
        return Ok(());
    };

    let response = agent
        .modify_group_membership(&request)
        .await
        .unwrap_or_else(|e| ModifyGroupMembershipResponse::Denied(backend(&e)));

    respond(&client, reply, &response).await
}

/// Answers as soon as the new policy is handed to the lease's proxy: the broker waits only
/// briefly for the reply, and the proxy picks the policy up within about a second.
async fn handle_network_policy_update_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
) -> anyhow::Result<()> {
    let Some((reply, request)) = accept::<NetworkPolicyUpdateRequest>(&msg, "network_policy_update") else {
        return Ok(());
    };
    let Some(response) = provider
        .update_network_policy(&request.lease_id, &request.network_policy)
        .await
    else {
        return Ok(());
    };
    respond(&client, reply, &response).await
}

async fn reach<Res: Serialize>(
    client: &Client,
    reply: &async_nats::Subject,
    provider: &FirecrackerProvider,
    lease_id: &LeaseId,
    deny: impl FnOnce(LeaseError) -> Res,
) -> anyhow::Result<Option<GuestAgent>> {
    match provider.access(lease_id).await {
        LeaseAccess::Live(agent) => Ok(Some(agent)),
        LeaseAccess::Lost(loss) => {
            respond(client, reply.clone(), &deny(lost(&loss))).await?;
            Ok(None)
        }
        LeaseAccess::NotHeld => Ok(None),
    }
}

fn lost(loss: &LeaseLoss) -> LeaseError {
    LeaseError::Backend {
        message: loss.to_string(),
    }
}

/// Decodes a request that expects a reply, discarding the ones that cannot produce one.
fn accept<Req: DeserializeOwned>(msg: &async_nats::Message, verb: &str) -> Option<(async_nats::Subject, Req)> {
    let reply = msg.reply.clone()?;
    match serde_json::from_slice(&msg.payload) {
        Ok(request) => Some((reply, request)),
        Err(e) => {
            warn!(verb, error = %e, "invalid request");
            None
        }
    }
}

async fn respond<T: Serialize>(client: &Client, reply: async_nats::Subject, value: &T) -> anyhow::Result<()> {
    let body = serde_json::to_vec(value)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

fn backend(error: &crate::agent::AgentError) -> LeaseError {
    LeaseError::Backend {
        message: error.to_string(),
    }
}

async fn handle_snapshot_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<FirecrackerProvider>,
    broadcast: bool,
) -> anyhow::Result<()> {
    let Some(reply) = msg.reply else {
        return Ok(());
    };
    let request: SnapshotExportRequest = match serde_json::from_slice(&msg.payload) {
        Ok(request) => request,
        Err(error) => {
            if !broadcast {
                respond(
                    &client,
                    reply,
                    &SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
                        message: format!("invalid snapshot request: {error}"),
                    }),
                )
                .await?;
            }
            return Ok(());
        }
    };
    if broadcast && !provider.answers_for(&request.lease_id).await {
        return Ok(());
    }
    let result = if let LeaseAccess::Lost(loss) = provider.access(&request.lease_id).await {
        SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
            message: loss.to_string(),
        })
    } else if request.target.artifact_type != trogon_sandbox_contract::ArtifactType::FirecrackerSnapshotV1 {
        SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
            message: "snapshot target must use FirecrackerSnapshotV1".into(),
        })
    } else {
        let exported = if broadcast {
            provider.checkpoint_to(&request.lease_id, &request.target).await
        } else {
            provider.snapshot_to(&request.lease_id, &request.target).await
        };
        match exported {
            Ok(receipt) => SnapshotExportOutcome::Ok(receipt),
            Err(error) if error.downcast_ref::<crate::retained::ReleaseExportError>().is_some() => {
                SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
                    message: error.to_string(),
                })
            }
            Err(error) => SnapshotExportOutcome::Err(SnapshotExportError::Transient {
                message: error.to_string(),
            }),
        }
    };
    respond(&client, reply, &result).await
}

#[cfg(test)]
fn extract_cancel_lease_id(subject: &str) -> Option<LeaseId> {
    let id = subject.rsplit('.').next()?;
    (!id.is_empty()).then(|| LeaseId::new(id))
}

/// Publishes a lifecycle event for a lease, naming the sandbox after the lease.
async fn publish_lease_event(
    js: &jetstream::Context,
    lease_id: &LeaseId,
    event: impl FnOnce(SandboxId) -> LifecycleEvent,
) {
    match SandboxId::new(lease_id.as_str()) {
        Ok(sandbox_id) => {
            publish_lifecycle(js, &lifecycle_subject(lease_id), &event(sandbox_id)).await;
        }
        Err(error) => warn!(lease_id = %lease_id, %error, "lease id is not a valid sandbox id"),
    }
}

async fn publish_lifecycle(js: &jetstream::Context, subject: &str, event: &LifecycleEvent) {
    match serde_json::to_vec(event) {
        Ok(payload) => {
            if let Err(e) = js.publish(subject.to_string(), payload.into()).await {
                warn!(subject, error = %e, "failed to publish lifecycle event");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize lifecycle event"),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod exit_marker_tests;

#[cfg(test)]
mod main_contract_tests;
