use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use std::time::SystemTime;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    AccountLimits, CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelOperationRequest,
    CancelOperationResponse, CommandExecution, CreateSandboxGroupResponse, CreateSandboxUserResponse,
    DeleteEntityResponse, DeleteNamedSandboxResponse, DeleteSnapshotTagResponse, Digest, DriveId, DriveRecord,
    DriveStorage, EntityRef, ExecutionId, FileOpRequest, FileOpResponse, GetEntityResponse, GetLeaseByNameResponse,
    GetSandboxDomainResponse, GroupMembershipAction, KillCommandResponse, LeaseError, LeaseGranted, LeaseId, LeaseName,
    LeaseReleasedReason, LeaseRequest, LeaseUsage, ListEntitiesRequest, ListEntitiesResponse, ListProcessesResponse,
    NativeHandleResponse, NativeSnapshotAcceptance, NativeSnapshotDeleteResponse, NativeSnapshotRef, NetworkPolicy,
    OperationCapabilities, OperationId, OperationRecord, OutputRetention, PauseLeaseResponse, PollCommandResponse,
    ProcessName, ReadLogsRequest, ReadLogsResponse, ReadyPorts, ReapLeasesRequest, ReapLeasesResponse,
    RestartCommandRequest, RestartCommandResponse, ResumeLeaseResponse, RunCommandRequest, RunCommandResponse,
    SandboxFileWrite, SandboxGroupName, SandboxTags, SandboxUserName, SnapshotDeleteResponse, SnapshotGetResponse,
    SnapshotLeaseResponse, SnapshotTagHistoryResponse, SnapshotTagName, SnapshotTreeResponse, SshAccessResponse,
    StdinChunk, StdinEof, StdinSeq, StopCommandRequest, StopCommandResponse, SuspendLeaseResponse, SuspendRequestLevel,
    TenantId, TenantUsage, TerminalId, TerminalOp, TerminalOutput, TerminalResponse, UnixSignal, WaitReadyResponse,
    WaitSandboxReadyResponse, WakeLeaseResponse, WriteStdinResponse,
};

use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;
use trogon_sandbox_client::{Bus, EvidenceStream, NatsBus, SandboxClient, TransportError};

use crate::evidence_event_body::EvidenceEventBody;
use crate::log_follow_body::LogFollowBody;
use crate::shared_bus::SharedBus;
use crate::{
    BackendError, CapabilitiesResponse, CapabilityRegistry, EventSender, NatsObservability, RunCommandOutcome,
    SandboxBackend, follow_logs_stream_id,
};

/// A [`SandboxBackend`] that forwards every call to the broker over a message bus.
pub struct NatsSandboxBackend {
    bus: Arc<dyn Bus>,
    capabilities: Arc<CapabilityRegistry>,
    observability: NatsObservability,
}

impl NatsSandboxBackend {
    pub fn new(
        client: async_nats::Client,
        capabilities: Arc<CapabilityRegistry>,
        observability: NatsObservability,
    ) -> Self {
        Self::from_bus(Arc::new(NatsBus::new(client)), capabilities, observability)
    }

    /// Builds a backend over any [`Bus`].
    pub fn from_bus(
        bus: Arc<dyn Bus>,
        capabilities: Arc<CapabilityRegistry>,
        observability: NatsObservability,
    ) -> Self {
        Self {
            bus,
            capabilities,
            observability,
        }
    }

    fn client(&self) -> SandboxClient {
        SandboxClient::from_bus(Box::new(SharedBus(Arc::clone(&self.bus))))
    }

    fn client_with_key(&self, api_key: &str) -> SandboxClient {
        self.client().with_api_key(api_key.to_string())
    }
}

#[async_trait]
impl SandboxBackend for NatsSandboxBackend {
    async fn operation_capabilities(
        &self,
        api_key: &str,
        tenant: TenantId,
    ) -> Result<OperationCapabilities, ClientError> {
        self.client_with_key(api_key).operation_capabilities(tenant).await
    }

    async fn cancel_operation(
        &self,
        api_key: &str,
        request: CancelOperationRequest,
    ) -> Result<CancelOperationResponse, ClientError> {
        self.client_with_key(api_key).cancel_operation_request(request).await
    }

    async fn get_operation(
        &self,
        api_key: &str,
        tenant: TenantId,
        operation_id: &OperationId,
    ) -> Result<Option<OperationRecord>, ClientError> {
        match self.client_with_key(api_key).get_operation(tenant, operation_id).await {
            Ok(operation) => Ok(Some(operation)),
            Err(ClientError::Denied(LeaseError::OperationNotFound { .. })) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn lease(
        &self,
        api_key: &str,
        request: &LeaseRequest,
        timeout: Duration,
    ) -> Result<LeaseGranted, ClientError> {
        self.client_with_key(api_key).lease(request, timeout).await
    }

    async fn cancel(&self, api_key: &str, lease_id: &LeaseId) -> Result<(), ClientError> {
        self.client_with_key(api_key).cancel(lease_id).await
    }

    async fn update_tags(&self, api_key: &str, lease_id: &LeaseId, tags: SandboxTags) -> Result<(), ClientError> {
        self.client_with_key(api_key).update_tags(lease_id, tags).await
    }

    async fn wait_released(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        timeout: Duration,
    ) -> Result<LeaseReleasedReason, ClientError> {
        self.client_with_key(api_key)
            .wait_released_with_reason(lease_id, timeout)
            .await
    }

    async fn stop_lease(&self, api_key: &str, tenant: TenantId, lease_id: &LeaseId) -> Result<(), ClientError> {
        self.client_with_key(api_key).stop_lease(tenant, lease_id).await
    }

    async fn pause_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
        max_paused_seconds: Option<u32>,
    ) -> Result<PauseLeaseResponse, ClientError> {
        self.client_with_key(api_key)
            .pause_lease(tenant, lease_id, max_paused_seconds)
            .await
    }

    async fn suspend_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
        level: SuspendRequestLevel,
        max_suspended_seconds: Option<u32>,
    ) -> Result<SuspendLeaseResponse, ClientError> {
        self.client_with_key(api_key)
            .suspend_lease(tenant, lease_id, level, max_suspended_seconds)
            .await
    }

    async fn wake_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
    ) -> Result<WakeLeaseResponse, ClientError> {
        self.client_with_key(api_key).wake_lease(tenant, lease_id).await
    }

    async fn snapshot_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
        accept_native: NativeSnapshotAcceptance,
    ) -> Result<SnapshotLeaseResponse, ClientError> {
        self.client_with_key(api_key)
            .snapshot_lease_accepting_native(tenant, lease_id, accept_native)
            .await
    }

    async fn snapshot_delete(
        &self,
        api_key: &str,
        tenant: TenantId,
        digest: Digest,
    ) -> Result<SnapshotDeleteResponse, ClientError> {
        self.client_with_key(api_key).snapshot_delete(tenant, digest).await
    }

    async fn native_snapshot_delete(
        &self,
        api_key: &str,
        tenant: TenantId,
        reference: NativeSnapshotRef,
    ) -> Result<NativeSnapshotDeleteResponse, ClientError> {
        self.client_with_key(api_key)
            .native_snapshot_delete(tenant, reference)
            .await
    }

    async fn resume_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
    ) -> Result<ResumeLeaseResponse, ClientError> {
        self.client_with_key(api_key).resume_lease(tenant, lease_id).await
    }

    async fn lease_usage(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
    ) -> Result<LeaseUsage, ClientError> {
        self.client_with_key(api_key).usage(tenant, lease_id).await
    }

    async fn tenant_usage(&self, api_key: &str, tenant: TenantId) -> Result<TenantUsage, ClientError> {
        self.client_with_key(api_key).tenant_usage(tenant).await
    }

    async fn account_limits(&self, api_key: &str, tenant: TenantId) -> Result<AccountLimits, ClientError> {
        self.client_with_key(api_key).limits(tenant).await
    }

    async fn delete_lease(&self, api_key: &str, tenant: TenantId, lease_id: &LeaseId) -> Result<(), ClientError> {
        self.client_with_key(api_key).delete_lease(tenant, lease_id).await
    }

    async fn extend_lease_timeout(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        additional_seconds: u32,
    ) -> Result<SystemTime, ClientError> {
        self.client_with_key(api_key)
            .extend_timeout(lease_id, additional_seconds)
            .await
    }

    async fn fork_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        source_lease_id: &LeaseId,
        ttl_seconds: u32,
        idempotency_key: String,
    ) -> Result<LeaseGranted, ClientError> {
        self.client_with_key(api_key)
            .fork(tenant, source_lease_id, ttl_seconds, idempotency_key)
            .await
    }

    async fn update_network_policy(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        network_policy: NetworkPolicy,
    ) -> Result<(), ClientError> {
        self.client_with_key(api_key)
            .update_network_policy(lease_id, network_policy)
            .await
    }

    async fn create_drive(
        &self,
        api_key: &str,
        tenant: TenantId,
        drive_id: DriveId,
        size_gib: Option<u64>,
        storage: DriveStorage,
    ) -> Result<DriveRecord, ClientError> {
        self.client_with_key(api_key)
            .create_drive_with_storage(tenant, drive_id, size_gib, storage)
            .await
    }

    async fn list_drives(&self, api_key: &str, tenant: TenantId) -> Result<Vec<DriveRecord>, ClientError> {
        self.client_with_key(api_key).list_drives(tenant).await
    }

    async fn delete_drive(&self, api_key: &str, tenant: TenantId, drive_id: DriveId) -> Result<(), ClientError> {
        self.client_with_key(api_key).delete_drive(tenant, drive_id).await
    }

    async fn run_command(
        &self,
        api_key: &str,
        mut request: RunCommandRequest,
    ) -> Result<RunCommandOutcome, ClientError> {
        let detached = matches!(request.execution, CommandExecution::Detached { .. });
        // A foreground command always runs with buffered output.
        if !detached {
            request.output_retention = OutputRetention::Buffer;
        }
        match self.client_with_key(api_key).send_run_command(request).await? {
            RunCommandResponse::Started(started) if detached => Ok(RunCommandOutcome::Started {
                execution_id: started.execution_id,
                origin: started.origin,
            }),
            RunCommandResponse::Completed(output) if !detached => Ok(RunCommandOutcome::Completed(output)),
            RunCommandResponse::Denied(e) => Err(ClientError::Denied(e)),
            other => Err(ClientError::Transport(TransportError::UnexpectedReply {
                operation: "run_command",
                reply: format!("{other:?} does not match detached={detached}"),
            })),
        }
    }

    async fn list_processes(&self, api_key: &str, lease_id: &LeaseId) -> Result<ListProcessesResponse, ClientError> {
        self.client_with_key(api_key).list_processes(lease_id).await
    }

    async fn wait_ready(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        timeout_seconds: u32,
    ) -> Result<WaitReadyResponse, ClientError> {
        self.client_with_key(api_key)
            .wait_ready(lease_id, execution_id, timeout_seconds)
            .await
    }

    async fn wait_sandbox_ready(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        ports: ReadyPorts,
        timeout_seconds: u32,
    ) -> Result<WaitSandboxReadyResponse, ClientError> {
        self.client_with_key(api_key)
            .wait_sandbox_ready(lease_id, ports, timeout_seconds)
            .await
    }

    async fn kill_process(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        name: &ProcessName,
        signal: UnixSignal,
    ) -> Result<KillCommandResponse, ClientError> {
        self.client_with_key(api_key)
            .kill_process_by_name(lease_id, name, signal)
            .await
    }

    async fn poll_command(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
    ) -> Result<PollCommandResponse, ClientError> {
        self.client_with_key(api_key).poll_command(lease_id, execution_id).await
    }

    async fn kill_command(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        signal: UnixSignal,
    ) -> Result<KillCommandResponse, ClientError> {
        self.client_with_key(api_key)
            .kill_command(lease_id, execution_id, signal)
            .await
    }

    async fn cancel_command_launch(
        &self,
        api_key: &str,
        request: CancelCommandLaunchRequest,
    ) -> Result<CancelCommandLaunchResponse, ClientError> {
        self.client_with_key(api_key).cancel_command_launch(request).await
    }

    async fn stop_command(
        &self,
        api_key: &str,
        request: StopCommandRequest,
    ) -> Result<StopCommandResponse, ClientError> {
        self.client_with_key(api_key)
            .stop_command(&request.lease_id, &request.execution_id, Some(request.grace_seconds))
            .await
    }

    async fn restart_command(
        &self,
        api_key: &str,
        request: RestartCommandRequest,
    ) -> Result<RestartCommandResponse, ClientError> {
        self.client_with_key(api_key)
            .restart_command(&request.lease_id, &request.execution_id, Some(request.grace_seconds))
            .await
    }

    async fn read_logs(&self, api_key: &str, request: ReadLogsRequest) -> Result<ReadLogsResponse, ClientError> {
        self.client_with_key(api_key)
            .read_logs(
                &request.lease_id,
                &request.execution_id,
                request.after_seq,
                request.max_bytes,
            )
            .await
    }

    async fn write_stdin(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        seq: StdinSeq,
        data: StdinChunk,
        eof: StdinEof,
    ) -> Result<WriteStdinResponse, ClientError> {
        self.client_with_key(api_key)
            .write_stdin(lease_id, execution_id, seq, data, eof)
            .await
    }

    async fn terminal(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        terminal_id: TerminalId,
        op: TerminalOp,
    ) -> Result<TerminalResponse, ClientError> {
        self.client_with_key(api_key).terminal(lease_id, terminal_id, op).await
    }

    async fn subscribe_terminal(
        &self,
        terminal_id: TerminalId,
        tx: EventSender,
        stop: CancellationToken,
    ) -> Result<(), BackendError> {
        let stream = self.client().terminal_output(&terminal_id).await?;
        tokio::spawn(forward_terminal_output(
            stream,
            terminal_id.as_str().to_string(),
            tx,
            stop,
        ));
        Ok(())
    }

    async fn write_files(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        files: Vec<SandboxFileWrite>,
        as_user: Option<SandboxUserName>,
    ) -> Result<(), ClientError> {
        self.client_with_key(api_key)
            .write_files(lease_id, files, as_user)
            .await
    }

    async fn read_file(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        path: String,
        as_user: Option<SandboxUserName>,
    ) -> Result<Vec<u8>, ClientError> {
        self.client_with_key(api_key).read_file(lease_id, path, as_user).await
    }

    async fn file_op(&self, api_key: &str, request: FileOpRequest) -> Result<FileOpResponse, ClientError> {
        self.client_with_key(api_key)
            .file_op(&request.lease_id, request.op, request.as_user)
            .await
    }

    async fn create_user(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        username: SandboxUserName,
    ) -> Result<CreateSandboxUserResponse, ClientError> {
        self.client_with_key(api_key).create_user(lease_id, username).await
    }

    async fn create_group(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        groupname: SandboxGroupName,
    ) -> Result<CreateSandboxGroupResponse, ClientError> {
        self.client_with_key(api_key).create_group(lease_id, groupname).await
    }

    async fn modify_group_membership(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        username: SandboxUserName,
        groupname: SandboxGroupName,
        action: GroupMembershipAction,
    ) -> Result<(), ClientError> {
        self.client_with_key(api_key)
            .modify_group_membership(lease_id, username, groupname, action)
            .await
    }

    async fn get_sandbox_domain(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        port: u16,
    ) -> Result<GetSandboxDomainResponse, ClientError> {
        self.client_with_key(api_key).get_sandbox_domain(lease_id, port).await
    }

    async fn native_handle(&self, api_key: &str, lease_id: &LeaseId) -> Result<NativeHandleResponse, ClientError> {
        self.client_with_key(api_key).native_handle(lease_id).await
    }

    async fn ssh_access(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        ttl_seconds: Option<u32>,
    ) -> Result<SshAccessResponse, ClientError> {
        self.client_with_key(api_key).ssh_access(lease_id, ttl_seconds).await
    }

    async fn get_entity(
        &self,
        api_key: &str,
        tenant: TenantId,
        entity: EntityRef,
    ) -> Result<GetEntityResponse, ClientError> {
        self.client_with_key(api_key).get_entity(tenant, entity).await
    }

    async fn list_entities(
        &self,
        api_key: &str,
        request: ListEntitiesRequest,
    ) -> Result<ListEntitiesResponse, ClientError> {
        self.client_with_key(api_key).list_entities(request).await
    }

    async fn delete_entity(
        &self,
        api_key: &str,
        tenant: TenantId,
        entity: EntityRef,
    ) -> Result<DeleteEntityResponse, ClientError> {
        self.client_with_key(api_key).delete_entity(tenant, entity).await
    }

    async fn get_lease_by_name(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: LeaseName,
    ) -> Result<GetLeaseByNameResponse, ClientError> {
        self.client_with_key(api_key).get_lease_by_name(tenant, name).await
    }

    async fn delete_named_sandbox(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: LeaseName,
    ) -> Result<DeleteNamedSandboxResponse, ClientError> {
        self.client_with_key(api_key).delete_named_sandbox(tenant, name).await
    }

    async fn snapshot_get(
        &self,
        api_key: &str,
        tenant: TenantId,
        digest: Digest,
    ) -> Result<SnapshotGetResponse, ClientError> {
        self.client_with_key(api_key).snapshot_get(tenant, digest).await
    }

    async fn snapshot_tree(
        &self,
        api_key: &str,
        tenant: TenantId,
        digest: Digest,
    ) -> Result<SnapshotTreeResponse, ClientError> {
        self.client_with_key(api_key).snapshot_tree(tenant, digest).await
    }

    async fn set_snapshot_tag(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: SnapshotTagName,
        digest: Digest,
    ) -> Result<(), ClientError> {
        self.client_with_key(api_key)
            .set_snapshot_tag(tenant, name, digest)
            .await
    }

    async fn delete_snapshot_tag(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: SnapshotTagName,
    ) -> Result<DeleteSnapshotTagResponse, ClientError> {
        self.client_with_key(api_key).delete_snapshot_tag(tenant, name).await
    }

    async fn snapshot_tag_history(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: SnapshotTagName,
    ) -> Result<SnapshotTagHistoryResponse, ClientError> {
        self.client_with_key(api_key).snapshot_tag_history(tenant, name).await
    }

    async fn list_capabilities(&self) -> CapabilitiesResponse {
        self.capabilities.snapshot().await
    }

    async fn follow_logs(
        &self,
        api_key: String,
        request: ReadLogsRequest,
        tx: EventSender,
    ) -> Result<(), BackendError> {
        let client = self.client_with_key(&api_key);
        tokio::spawn(forward_log_follow(client, request, tx));
        Ok(())
    }

    async fn subscribe_evidence(
        &self,
        api_key: String,
        lease_id: LeaseId,
        tx: EventSender,
    ) -> Result<(), BackendError> {
        let stream = self.client_with_key(&api_key).evidence_stream(&lease_id).await?;
        tokio::spawn(forward_evidence(stream, lease_id.as_str().to_string(), tx));
        Ok(())
    }

    async fn list_leases(
        &self,
        tenant: &TenantId,
        tag_filter: &BTreeMap<String, String>,
    ) -> Result<Value, BackendError> {
        Ok(self.observability.list_leases(tenant, tag_filter).await?)
    }

    async fn reap_leases(&self, api_key: &str, request: ReapLeasesRequest) -> Result<ReapLeasesResponse, ClientError> {
        self.client_with_key(api_key).reap_leases(request).await
    }

    async fn list_snapshots(&self, tenant: &TenantId) -> Result<Value, BackendError> {
        Ok(self.observability.list_snapshots(tenant).await?)
    }

    async fn list_tags(&self, tenant: &TenantId) -> Result<Value, BackendError> {
        Ok(self.observability.list_tags(tenant).await?)
    }

    fn spawn_dashboard_watchers(&self, tenant: TenantId, tx: EventSender, cancel: CancellationToken) {
        self.observability
            .spawn_lease_watcher(tenant.clone(), tx.clone(), cancel.clone());
        self.observability
            .spawn_snapshot_published_watcher(tenant, tx.clone(), cancel.clone());
        self.observability.spawn_worker_heartbeat_watcher(tx, cancel);
    }
}

async fn forward_terminal_output(
    mut stream: BoxStream<'static, TerminalOutput>,
    stream_id: String,
    tx: EventSender,
    stop: CancellationToken,
) {
    loop {
        let frame = tokio::select! {
            () = stop.cancelled() => break,
            frame = stream.next() => frame,
        };
        let Some(frame) = frame else { break };
        let ended = matches!(frame, TerminalOutput::Exit { .. });
        let event = serde_json::to_value(&frame);
        let Ok(event) = event else { continue };
        if tx.send((stream_id.clone(), event)).await.is_err() || ended {
            break;
        }
    }
}

async fn forward_log_follow(client: SandboxClient, request: ReadLogsRequest, tx: EventSender) {
    let stream_id = follow_logs_stream_id(&request.lease_id, &request.execution_id);
    let mut stream = client.follow_logs(&request.lease_id, request.execution_id.clone(), request.after_seq);
    loop {
        tokio::select! {
            biased;
            item = stream.next() => {
                let Some(item) = item else { break };
                let Ok(event) = serde_json::to_value(LogFollowBody::from(&item)) else { continue };
                if tx.send((stream_id.clone(), event)).await.is_err() {
                    break;
                }
            }
            () = tx.closed() => break,
        }
    }
}

async fn forward_evidence(mut stream: EvidenceStream, stream_id: String, tx: EventSender) {
    while let Some(Ok(frame)) = stream.next().await {
        let event = serde_json::to_value(EvidenceEventBody::from(frame));
        let Ok(event) = event else { continue };
        if tx.send((stream_id.clone(), event)).await.is_err() {
            break;
        }
    }
}
