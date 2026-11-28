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
mod encode_within;
mod handler_error;
mod serve;
mod serve_error;
mod verb;
mod worker_args;
mod worker_error;
mod workspace_files;

pub use serve::serve;
pub use serve_error::ServeError;
pub use worker_args::WorkerArgs;
pub use worker_error::WorkerError;

use std::sync::{Arc, OnceLock};

use async_nats::Client;
use async_nats::jetstream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::{BoxStream, select_all};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    CancelIntent, CapabilitySupport, CheckpointRunningRequest, CommandDeadline, CommandExecution, CommandOutputChunk,
    CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse,
    ExecutionId, FileOpRequest, GetSandboxDomainRequest, KillCommandRequest, KillCommandResponse, LeaseError, LeaseId,
    LifecycleEvent, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse, NetworkPolicyUpdateRequest,
    NetworkPolicyUpdateResponse, PollCommandRequest, PollCommandResponse, ProviderDispatchPayload,
    ProviderDispatchReply, ProviderKind, ProviderLease, PurgeNamedRequest, PurgeNamedResponse, RunCommandRequest,
    RunCommandResponse, SandboxId, SnapshotExportError, SnapshotExportOutcome, WorkerCapabilities, WorkerCapacity,
    WorkerHeartbeat, WorkerId, WriteStdinRequest, WriteStdinResponse,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityMode, DEFAULT_HEARTBEAT_INTERVAL, DETACHED_EXIT_POLL_FAILURES,
    DETACHED_EXIT_POLL_INTERVAL, NatsAuthorityOracle, Provider, ProviderError, RedispatchGuard, await_detached_exit,
    decode_verified, dispatch_failure_reply, encode_file_op_response_within, lifecycle_subject, nats_transfer,
    publish_command_exit, queue_subscribe_verified, spawn_heartbeat_loop, subjects, subscribe_verified,
    unsupported_file_op_response,
};
use trogon_std::env::ReadEnv;

use crate::adapter::{PurgeOutcome, SubstrateProvider};
use crate::agent::{ActorAgent, ChunkSink, DiscardChunks};

use constants::{LEASE_QUEUE_GROUP, PURGE_NAMED_QUEUE_GROUP};
use encode_within::{encode_poll_within, encode_run_within};
use handler_error::HandlerError;
use verb::Verb;
use workspace_files::{handle_read_file_msg, handle_write_files_msg};

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
    /// The agent's execution id, learned from the first frame.
    execution_id: OnceLock<ExecutionId>,
}

impl NatsChunkSink {
    fn new(js: jetstream::Context, lease_id: LeaseId) -> Self {
        Self {
            js,
            lease_id,
            execution_id: OnceLock::new(),
        }
    }

    /// Announces that the streamed command exited, after its last chunk, so
    /// log followers end at once instead of polling for the exit. The agent
    /// names the execution in a begin frame even when the command prints
    /// nothing; only an agent that predates that frame leaves a silent
    /// command unnamed, and then there is nothing to announce.
    async fn publish_exit(&self, exit_code: Option<i32>) {
        let Some(execution_id) = self.execution_id.get() else {
            return;
        };
        publish_exit_marker(&self.js, &self.lease_id, execution_id, exit_code).await;
    }
}

async fn publish_exit_marker(
    js: &jetstream::Context,
    lease_id: &LeaseId,
    execution_id: &ExecutionId,
    exit_code: Option<i32>,
) {
    if let Err(error) = publish_command_exit(js, lease_id, execution_id, exit_code).await {
        warn!(
            lease_id = %lease_id,
            execution_id = %execution_id,
            %error,
            "failed to publish command exit marker"
        );
    }
}

/// Watches a detached streamed command until it exits and then announces the
/// exit, since its `Started` reply leaves nothing on the request path to see it.
fn watch_detached_exit(agent: ActorAgent, js: jetstream::Context, lease_id: LeaseId, execution_id: ExecutionId) {
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
        publish_exit_marker(&js, &lease_id, &execution_id, exit.exit_code()).await
    });
}

#[async_trait]
impl ChunkSink for NatsChunkSink {
    async fn begin(&self, execution_id: String) {
        let _ = self.execution_id.set(ExecutionId::from(execution_id));
    }

    async fn publish(&self, chunk: CommandOutputChunk) {
        self.execution_id.get_or_init(|| chunk.execution_id.clone());
        let subject = chunk_subject(&self.lease_id, &chunk.execution_id);
        publish_json(&self.js, &subject, &chunk, "command output chunk").await;
    }
}

/// Serves until `cancel` fires, under the authority mode named in `env`.
pub async fn run(
    client: Client,
    js: jetstream::Context,
    provider: Arc<SubstrateProvider>,
    worker_id: WorkerId,
    cancel: CancellationToken,
    env: &impl ReadEnv,
) -> Result<(), WorkerError> {
    let authority = AuthorityConfig::new(AuthorityMode::from_env(env)?);
    run_with_authority(client, js, provider, worker_id, cancel, authority).await
}

/// Serves until `cancel` fires, under an explicit authority configuration.
pub async fn run_with_authority(
    client: Client,
    js: jetstream::Context,
    provider: Arc<SubstrateProvider>,
    worker_id: WorkerId,
    cancel: CancellationToken,
    authority: AuthorityConfig,
) -> Result<(), WorkerError> {
    let owner = provider.worker_id().clone();
    if worker_id != owner {
        return Err(WorkerError::WorkerIdMismatch { worker_id, owner });
    }
    let events = subscribe_all(&client, &worker_id).await?;
    let grants = Arc::new(RedispatchGuard::new());
    let guard = AuthorityGuard::new(
        authority,
        owner.clone(),
        Arc::new(NatsAuthorityOracle::new(client.clone(), owner)),
        provider.clone(),
    );
    let stop_authority = cancel.child_token();
    let authority_task = tokio::spawn(guard.run(stop_authority.clone()));

    info!(%worker_id, "worker ready");

    let mut events = events;
    while let Some((verb, msg)) = tokio::select! {
        _ = cancel.cancelled() => None,
        event = events.next() => event,
    } {
        let (client, js) = (client.clone(), js.clone());
        let (provider, grants) = (Arc::clone(&provider), Arc::clone(&grants));
        tokio::spawn(async move {
            report_failure(verb, serve_message(verb, msg, client, js, provider, grants).await);
        });
    }
    info!("worker shutting down");

    stop_authority.cancel();
    authority_task.await?;
    Ok(())
}

/// Every subscription the worker serves, tagged with the verb its messages belong to.
async fn subscribe_all(
    client: &Client,
    worker_id: &WorkerId,
) -> Result<futures::stream::SelectAll<BoxStream<'static, (Verb, async_nats::Message)>>, WorkerError> {
    let kind = ProviderKind::Substrate;
    let all = LeaseId::new(">");
    let streams: Vec<BoxStream<'static, (Verb, async_nats::Message)>> = vec![
        queue_subscribe_verified(client, subjects::provider_dispatch(kind), LEASE_QUEUE_GROUP)
            .await?
            .map(|msg| (Verb::Lease, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_cancel(kind, &all))
            .await?
            .map(|msg| (Verb::Cancel, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_run_command(kind, &all))
            .await?
            .map(|msg| (Verb::RunCommand, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_poll_command(kind, &all))
            .await?
            .map(|msg| (Verb::PollCommand, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_kill_command(kind, &all))
            .await?
            .map(|msg| (Verb::KillCommand, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_write_stdin(kind, &all))
            .await?
            .map(|msg| (Verb::WriteStdin, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_write_files(kind, &all))
            .await?
            .map(|msg| (Verb::WriteFiles, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_read_file(kind, &all))
            .await?
            .map(|msg| (Verb::ReadFile, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_file_op(kind, &all))
            .await?
            .map(|msg| (Verb::FileOp, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_get_domain(kind, &all))
            .await?
            .map(|msg| (Verb::GetDomain, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_create_user(kind, &all))
            .await?
            .map(|msg| (Verb::CreateUser, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_create_group(kind, &all))
            .await?
            .map(|msg| (Verb::CreateGroup, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_modify_group_membership(kind, &all))
            .await?
            .map(|msg| (Verb::ModifyGroupMembership, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_checkpoint_running(kind, &all))
            .await?
            .map(|msg| (Verb::CheckpointRunning, msg))
            .boxed(),
        subscribe_verified(client, subjects::provider_network_policy_update(kind, &all))
            .await?
            .map(|msg| (Verb::NetworkPolicyUpdate, msg))
            .boxed(),
        subscribe_verified(client, subjects::worker_snapshot_export(worker_id))
            .await?
            .map(|msg| (Verb::SnapshotExport, msg))
            .boxed(),
        queue_subscribe_verified(client, subjects::provider_purge_named(kind), PURGE_NAMED_QUEUE_GROUP)
            .await?
            .map(|msg| (Verb::PurgeNamed, msg))
            .boxed(),
    ];
    Ok(select_all(streams))
}

/// Logs a request that could not be answered; the requester times out and may retry.
fn report_failure(verb: Verb, outcome: Result<(), HandlerError>) {
    if let Err(error) = outcome {
        warn!(?verb, %error, "failed to handle request");
    }
}

/// Routes one message to the handler for the subscription it arrived on.
async fn serve_message(
    verb: Verb,
    msg: async_nats::Message,
    client: Client,
    js: jetstream::Context,
    provider: Arc<SubstrateProvider>,
    grants: Arc<RedispatchGuard>,
) -> Result<(), HandlerError> {
    match verb {
        Verb::Lease => handle_lease_msg(msg, client, js, provider, grants).await,
        Verb::Cancel => {
            handle_cancel_msg(&msg, provider.as_ref(), grants.as_ref()).await;
            Ok(())
        }
        Verb::RunCommand => handle_run_command_msg(msg, client, js, provider).await,
        Verb::PollCommand => handle_poll_command_msg(msg, client, provider).await,
        Verb::KillCommand => handle_kill_command_msg(msg, client, provider).await,
        Verb::WriteStdin => handle_write_stdin_msg(msg, client, provider).await,
        Verb::WriteFiles => handle_write_files_msg(msg, client, provider).await,
        Verb::ReadFile => handle_read_file_msg(msg, client, provider).await,
        Verb::FileOp => handle_file_op_msg(msg, client, provider).await,
        Verb::GetDomain => handle_get_domain_msg(msg, client, provider).await,
        Verb::CreateUser => handle_create_user_msg(msg, client, provider).await,
        Verb::CreateGroup => handle_create_group_msg(msg, client, provider).await,
        Verb::ModifyGroupMembership => handle_modify_group_membership_msg(msg, client, provider).await,
        Verb::CheckpointRunning => {
            let refused = SnapshotExportOutcome::Err(checkpoint_running_denial());
            refuse::<CheckpointRunningRequest, _>(
                msg,
                client,
                provider,
                "checkpoint_running",
                |r| r.lease_id.clone(),
                refused,
            )
            .await
        }
        Verb::NetworkPolicyUpdate => {
            let refused = NetworkPolicyUpdateResponse::Denied(network_policy_denial());
            refuse::<NetworkPolicyUpdateRequest, _>(
                msg,
                client,
                provider,
                "network_policy_update",
                |r| r.lease_id.clone(),
                refused,
            )
            .await
        }
        Verb::SnapshotExport => handle_snapshot_msg(msg, client).await,
        Verb::PurgeNamed => handle_purge_named_msg(msg, client, provider).await,
    }
}

/// Publishes capacity until `cancel` fires.
///
/// `current_leases` comes from the provider's own map, which recovery has already
/// repopulated, so a restarted worker does not briefly advertise itself as empty and attract
/// every pending lease.
pub fn spawn_heartbeat_task(client: Client, provider: Arc<SubstrateProvider>, cancel: CancellationToken) {
    let worker_id = provider.worker_id().clone();
    spawn_heartbeat_loop(
        client,
        ProviderKind::Substrate,
        worker_id.clone(),
        DEFAULT_HEARTBEAT_INTERVAL,
        cancel,
        move || {
            let (worker_id, provider) = (worker_id.clone(), provider.clone());
            async move {
                build_heartbeat(
                    &worker_id,
                    provider.config().max_concurrent_leases,
                    provider.current_lease_count().await,
                )
            }
        },
    );
}

pub(crate) fn build_heartbeat(
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    current_leases: u32,
) -> WorkerHeartbeat {
    let profile = crate::profile::PROFILE.clone();
    WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::Substrate,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: std::time::SystemTime::now(),
        // Substrate caches images as actor templates, keyed by a workload digest of our own
        // making rather than by the image digest the broker matches on. Reporting an empty
        // set keeps the broker from routing a lease here for a warmth this worker cannot
        // prove it has.
        cached_digests: Vec::new(),
        profile,
        warm_pool_population: std::collections::HashMap::new(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Vec::new(),
    }
}

async fn handle_lease_msg<P: Provider + ?Sized>(
    msg: async_nats::Message,
    client: Client,
    js: jetstream::Context,
    provider: Arc<P>,
    grants: Arc<RedispatchGuard>,
) -> Result<(), HandlerError> {
    let reply = match decode_verified::<ProviderDispatchPayload>(&msg.payload, None) {
        Err(error) => {
            warn!(%error, "invalid dispatch payload");
            ProviderDispatchReply::DispatchRejected {
                decode_error: error.to_string(),
            }
        }
        Ok(payload) => {
            info!(
                lease_id = %payload.lease_id,
                tenant = %payload.request.tenant,
                "lease accepted"
            );
            // A redelivered dispatch gets the first grant, so the lease's lifecycle stream sees one
            // Started rather than one per redelivery.
            let result = grants
                .get_or_launch(&payload.lease_id, || {
                    launch_with_lifecycle(&js, &payload, provider.as_ref())
                })
                .await;
            match result {
                Ok(lease) => ProviderDispatchReply::Granted(lease),
                Err(error) => dispatch_failure_reply(&error),
            }
        }
    };

    match msg.reply {
        Some(subject) => respond(&client, subject, &reply).await,
        None => Ok(()),
    }
}

/// Launches the lease and brackets it with Started and Completed or Failed on its
/// lifecycle stream. Every phase in between is emitted by the provider itself, which is the
/// only thing that knows which of them a given lease actually goes through.
///
/// A lease id that is not a valid sandbox id has no lifecycle stream to bracket, so it is
/// launched without one.
async fn launch_with_lifecycle<P: Provider + ?Sized>(
    js: &jetstream::Context,
    payload: &ProviderDispatchPayload,
    provider: &P,
) -> Result<ProviderLease, ProviderError> {
    let subject = lifecycle_subject(&payload.lease_id);
    let launch_id = SandboxId::new(payload.lease_id.as_str()).ok();
    if let Some(sandbox_id) = &launch_id {
        let started = LifecycleEvent::Started {
            sandbox_id: sandbox_id.clone(),
        };
        publish_lifecycle(js, &subject, &started).await;
    }

    let result = provider.lease(&payload.request, &payload.lease_id).await;

    match &result {
        Ok(lease) => {
            info!(
                lease_id = %payload.lease_id,
                sandbox_id = %lease.sandbox_id,
                tenant = %payload.request.tenant,
                "sandbox launched"
            );
            let completed = LifecycleEvent::Completed {
                sandbox_id: lease.sandbox_id.clone(),
            };
            publish_lifecycle(js, &subject, &completed).await;
        }
        Err(error) => {
            warn!(
                lease_id = %payload.lease_id,
                tenant = %payload.request.tenant,
                %error,
                "sandbox launch failed"
            );
            if let Some(sandbox_id) = launch_id {
                let failed = LifecycleEvent::Failed {
                    sandbox_id,
                    error: error.to_string(),
                };
                publish_lifecycle(js, &subject, &failed).await;
            }
        }
    }
    result
}

/// What a cancel body asks for. An empty body is a plain release; a body that is present but
/// unreadable is refused, because guessing a release would silently keep what a purge was
/// meant to discard.
fn cancel_intent(body: &[u8]) -> Result<CancelIntent, serde_json::Error> {
    if body.is_empty() {
        return Ok(CancelIntent::Release);
    }
    serde_json::from_slice(body)
}

async fn handle_cancel_msg(msg: &async_nats::Message, provider: &SubstrateProvider, grants: &RedispatchGuard) {
    let Some(lease_id) = extract_cancel_lease_id(msg.subject.as_str()) else {
        return;
    };
    // A reused lease id after a cancel is a new launch, not a redelivery.
    grants.forget(&lease_id);

    // `cancel` is a no-op for a lease this worker does not hold, so the broadcast needs no
    // guard here: the provider is the one that knows.
    let intent = match cancel_intent(&msg.payload) {
        Ok(intent) => intent,
        Err(error) => {
            warn!(lease_id = %lease_id, %error, "cancel body is unreadable; refusing to guess its intent");
            return;
        }
    };
    let outcome = match intent {
        CancelIntent::Release => provider.cancel(&lease_id).await,
        CancelIntent::Purge => provider.purge(&lease_id).await,
    };
    match outcome {
        Ok(()) => info!(lease_id = %lease_id, "lease released"),
        Err(error) => warn!(lease_id = %lease_id, %error, "cancel failed"),
    }
}

async fn handle_run_command_msg(
    msg: async_nats::Message,
    client: Client,
    js: jetstream::Context,
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<RunCommandRequest>(&msg, "run_command") else {
        return Ok(());
    };

    if let Some(reason) = run_command_spec_violation(&request) {
        if provider.active_lease(&request.lease_id).await.is_none() {
            return Ok(());
        }
        let response = RunCommandResponse::Denied(LeaseError::InvalidSpec { reason });
        return respond(&client, reply, &response).await;
    }

    let Some(agent) = provider.agent_for(&request.lease_id).await else {
        return Ok(());
    };

    let response = exec_streamed(&agent, js, &request).await;

    let body = encode_run_within(response, client.server_info().max_payload)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

/// Runs a command on the agent, publishing its output and exit marker on the evidence
/// stream when it asked to stream.
async fn exec_streamed(agent: &ActorAgent, js: jetstream::Context, request: &RunCommandRequest) -> RunCommandResponse {
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
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<PollCommandRequest>(&msg, "poll_command") else {
        return Ok(());
    };

    let Some(agent) = provider.agent_for(&request.lease_id).await else {
        return Ok(());
    };

    let response = agent
        .poll(&request)
        .await
        .unwrap_or_else(|e| PollCommandResponse::Denied(backend(&e)));

    let body = encode_poll_within(response, client.server_info().max_payload)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

async fn handle_kill_command_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<KillCommandRequest>(&msg, "kill_command") else {
        return Ok(());
    };

    let Some(agent) = provider.agent_for(&request.lease_id).await else {
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
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<WriteStdinRequest>(&msg, "write_stdin") else {
        return Ok(());
    };

    let Some(agent) = provider.agent_for(request.lease_id()).await else {
        return Ok(());
    };

    let response = agent
        .write_stdin(&request)
        .await
        .unwrap_or_else(|e| WriteStdinResponse::Denied(backend(&e)));

    respond(&client, reply, &response).await
}

/// Why this provider cannot run `request`, when the request asks for something it does not do.
///
/// An attached command cannot carry interactive stdin, a lease deadline or a process spec: the
/// request type refuses to decode with them, so only a detached command's deadline is left to
/// check here.
fn run_command_spec_violation(request: &RunCommandRequest) -> Option<String> {
    if request.require_launch_cancellation.is_required() {
        return Some("command launch cancellation is not supported by this provider".to_string());
    }
    match &request.execution {
        CommandExecution::Detached {
            deadline: CommandDeadline::Lease,
            ..
        } => Some("lease deadline is not supported by this provider".to_string()),
        _ => None,
    }
}

/// Answers a `file_op` request for a lease this worker holds with a typed `Unsupported`
/// reply, so the caller learns the provider cannot serve it instead of timing out.
async fn handle_file_op_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &msg).await?;
    let request: FileOpRequest = match serde_json::from_slice(&payload) {
        Ok(request) => request,
        Err(error) => {
            warn!(%error, "invalid file_op request");
            return Ok(());
        }
    };
    if provider.agent_for(&request.lease_id).await.is_none() {
        return Ok(());
    }
    let response = unsupported_file_op_response("substrate", &request.op);
    let body = encode_file_op_response_within(&response, nats_transfer::MAX_TRANSFER_BYTES)?;
    nats_transfer::respond(&client, reply.as_str(), body.into()).await?;
    Ok(())
}

async fn handle_create_user_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<CreateSandboxUserRequest>(&msg, "create_user") else {
        return Ok(());
    };

    let Some(agent) = provider.agent_for(&request.lease_id).await else {
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
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<CreateSandboxGroupRequest>(&msg, "create_group") else {
        return Ok(());
    };

    let Some(agent) = provider.agent_for(&request.lease_id).await else {
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
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<ModifyGroupMembershipRequest>(&msg, "modify_group_membership") else {
        return Ok(());
    };

    let Some(agent) = provider.agent_for(&request.lease_id).await else {
        return Ok(());
    };

    let response = agent
        .modify_group_membership(&request)
        .await
        .unwrap_or_else(|e| ModifyGroupMembershipResponse::Denied(backend(&e)));

    respond(&client, reply, &response).await
}

async fn handle_get_domain_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<GetSandboxDomainRequest>(&msg, "get_domain") else {
        return Ok(());
    };

    if provider.active_lease(&request.lease_id).await.is_none() {
        return Ok(());
    }

    let response = provider.sandbox_domain(&request.lease_id, request.port).await;
    respond(&client, reply, &response).await
}

async fn handle_purge_named_msg(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    let Some((reply, request)) = accept::<PurgeNamedRequest>(&msg, "purge_named") else {
        return Ok(());
    };

    let response = match provider.purge_named(&request.tenant, request.name.as_str()).await {
        Ok(PurgeOutcome::Purged) => PurgeNamedResponse::Purged,
        Ok(PurgeOutcome::NotFound) => PurgeNamedResponse::NotFound,
        Err(error) => PurgeNamedResponse::Failed {
            message: error.to_string(),
        },
    };

    respond(&client, reply, &response).await
}

/// Refuses a snapshot export. It is addressed to this worker by id, so it is answered without
/// consulting the active map: there is no other worker that could answer instead.
async fn handle_snapshot_msg(msg: async_nats::Message, client: Client) -> Result<(), HandlerError> {
    match msg.reply {
        Some(reply) => respond(&client, reply, &SnapshotExportOutcome::Err(snapshot_denial())).await,
        None => Ok(()),
    }
}

/// Answers a broadcast this provider does not serve, but only for a lease it holds.
async fn refuse<Req, Res>(
    msg: async_nats::Message,
    client: Client,
    provider: Arc<SubstrateProvider>,
    verb: &str,
    lease_of: impl Fn(&Req) -> LeaseId,
    response: Res,
) -> Result<(), HandlerError>
where
    Req: DeserializeOwned,
    Res: Serialize,
{
    let Some((reply, request)) = accept::<Req>(&msg, verb) else {
        return Ok(());
    };

    if provider.active_lease(&lease_of(&request)).await.is_none() {
        return Ok(());
    }

    respond(&client, reply, &response).await
}

/// Decodes a request that expects a reply, discarding the ones that cannot produce one.
fn accept<Req: DeserializeOwned>(msg: &async_nats::Message, verb: &str) -> Option<(async_nats::Subject, Req)> {
    let reply = msg.reply.clone()?;
    match serde_json::from_slice(&msg.payload) {
        Ok(request) => Some((reply, request)),
        Err(error) => {
            warn!(verb, %error, "invalid request");
            None
        }
    }
}

async fn respond<T: Serialize>(client: &Client, reply: async_nats::Subject, value: &T) -> Result<(), HandlerError> {
    let body = serde_json::to_vec(value)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}

fn backend(error: &crate::agent::AgentError) -> LeaseError {
    LeaseError::Backend {
        message: error.to_string(),
    }
}

fn checkpoint_running_denial() -> SnapshotExportError {
    SnapshotExportError::Unsupported {
        message: "substrate snapshots are internal to the runtime and cannot be exported".to_string(),
    }
}

fn snapshot_denial() -> SnapshotExportError {
    checkpoint_running_denial()
}

fn network_policy_denial() -> LeaseError {
    LeaseError::InvalidNetworkPolicy {
        reason: "the substrate provider does not program egress policy".to_string(),
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
    publish_json(js, subject, event, "lifecycle event").await;
}

/// Publishes `value` as JSON, logging instead of failing: evidence is best effort and must not
/// interrupt the command or the launch it describes.
async fn publish_json<T: Serialize>(js: &jetstream::Context, subject: &str, value: &T, what: &str) {
    match serde_json::to_vec(value) {
        Ok(payload) => {
            if let Err(error) = js.publish(subject.to_string(), payload.into()).await {
                warn!(subject, what, %error, "failed to publish");
            }
        }
        Err(error) => warn!(subject, what, %error, "failed to serialize"),
    }
}

#[cfg(test)]
mod tests;
