use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use std::time::SystemTime;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    AccountLimits, CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelOperationRequest,
    CancelOperationResponse, CreateSandboxGroupResponse, CreateSandboxUserResponse, DeleteEntityResponse,
    DeleteNamedSandboxResponse, DeleteSnapshotTagResponse, Digest, DriveId, DriveRecord, DriveStorage, EntityRef,
    ExecutionId, FileOpRequest, FileOpResponse, GetEntityResponse, GetLeaseByNameResponse, GetSandboxDomainResponse,
    GroupMembershipAction, KillCommandResponse, LeaseGranted, LeaseId, LeaseName, LeaseReleasedReason, LeaseRequest,
    LeaseUsage, ListEntitiesRequest, ListEntitiesResponse, ListProcessesResponse, NativeHandleResponse,
    NativeSnapshotAcceptance, NativeSnapshotDeleteResponse, NativeSnapshotRef, NetworkPolicy, OperationCapabilities,
    OperationId, OperationRecord, PauseLeaseResponse, PollCommandResponse, ProcessName, ReadLogsRequest,
    ReadLogsResponse, ReadyPorts, ReapLeasesRequest, ReapLeasesResponse, RestartCommandRequest, RestartCommandResponse,
    ResumeLeaseResponse, RunCommandRequest, SandboxFileWrite, SandboxGroupName, SandboxTags, SandboxUserName,
    SnapshotDeleteResponse, SnapshotGetResponse, SnapshotLeaseResponse, SnapshotTagHistoryResponse, SnapshotTagName,
    SnapshotTreeResponse, SshAccessResponse, StdinChunk, StdinEof, StdinSeq, StopCommandRequest, StopCommandResponse,
    SuspendLeaseResponse, SuspendRequestLevel, TenantId, TenantUsage, TerminalId, TerminalOp, TerminalResponse,
    UnixSignal, WaitReadyResponse, WaitSandboxReadyResponse, WakeLeaseResponse, WriteStdinResponse,
};

use crate::empty_listing::empty_listing;
use crate::{BackendError, CapabilitiesResponse, EventSender, RunCommandOutcome};

#[async_trait]
pub trait SandboxBackend: Send + Sync {
    async fn operation_capabilities(
        &self,
        api_key: &str,
        tenant: TenantId,
    ) -> Result<OperationCapabilities, ClientError>;

    async fn cancel_operation(
        &self,
        api_key: &str,
        request: CancelOperationRequest,
    ) -> Result<CancelOperationResponse, ClientError>;

    async fn get_operation(
        &self,
        api_key: &str,
        tenant: TenantId,
        operation_id: &OperationId,
    ) -> Result<Option<OperationRecord>, ClientError>;

    async fn lease(
        &self,
        api_key: &str,
        request: &LeaseRequest,
        timeout: Duration,
    ) -> Result<LeaseGranted, ClientError>;

    async fn cancel(&self, api_key: &str, lease_id: &LeaseId) -> Result<(), ClientError>;

    /// Fully replaces `lease_id`'s tag set with `tags` (not a merge).
    async fn update_tags(&self, api_key: &str, lease_id: &LeaseId, tags: SandboxTags) -> Result<(), ClientError>;

    async fn wait_released(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        timeout: Duration,
    ) -> Result<LeaseReleasedReason, ClientError>;

    /// Stops `lease_id`'s current session while keeping it resumable later.
    async fn stop_lease(&self, api_key: &str, tenant: TenantId, lease_id: &LeaseId) -> Result<(), ClientError>;

    /// Pauses `lease_id` in place, keeping its id and state.
    async fn pause_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
        max_paused_seconds: Option<u32>,
    ) -> Result<PauseLeaseResponse, ClientError>;

    /// Suspends `lease_id`, releasing host resources while keeping its id and state.
    async fn suspend_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
        level: SuspendRequestLevel,
        max_suspended_seconds: Option<u32>,
    ) -> Result<SuspendLeaseResponse, ClientError>;

    /// Wakes a suspended `lease_id`.
    async fn wake_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
    ) -> Result<WakeLeaseResponse, ClientError>;

    /// Snapshots running `lease_id` without stopping it. `accept_native` lets a
    /// provider-native receipt come back; clients that omit it get `Unsupported` there.
    async fn snapshot_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
        accept_native: NativeSnapshotAcceptance,
    ) -> Result<SnapshotLeaseResponse, ClientError>;

    /// Deletes a snapshot by digest.
    async fn snapshot_delete(
        &self,
        api_key: &str,
        tenant: TenantId,
        digest: Digest,
    ) -> Result<SnapshotDeleteResponse, ClientError>;

    async fn native_snapshot_delete(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _reference: NativeSnapshotRef,
    ) -> Result<NativeSnapshotDeleteResponse, ClientError> {
        Ok(NativeSnapshotDeleteResponse::Unsupported)
    }

    /// Resumes a paused `lease_id`.
    async fn resume_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
    ) -> Result<ResumeLeaseResponse, ClientError>;

    /// Usage counters for `lease_id`.
    async fn lease_usage(&self, api_key: &str, tenant: TenantId, lease_id: &LeaseId)
    -> Result<LeaseUsage, ClientError>;

    /// Counters summed over `tenant`'s live leases.
    async fn tenant_usage(&self, api_key: &str, tenant: TenantId) -> Result<TenantUsage, ClientError>;

    /// `tenant`'s limits and current consumption.
    async fn account_limits(&self, api_key: &str, tenant: TenantId) -> Result<AccountLimits, ClientError>;

    /// Permanently deletes `lease_id`, purging any resumable state.
    async fn delete_lease(&self, api_key: &str, tenant: TenantId, lease_id: &LeaseId) -> Result<(), ClientError>;

    /// Extends `lease_id`'s wall-clock deadline by `additional_seconds`.
    async fn extend_lease_timeout(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        additional_seconds: u32,
    ) -> Result<SystemTime, ClientError>;

    /// Forks `source_lease_id` into a brand-new, independent lease.
    async fn fork_lease(
        &self,
        api_key: &str,
        tenant: TenantId,
        source_lease_id: &LeaseId,
        ttl_seconds: u32,
        idempotency_key: String,
    ) -> Result<LeaseGranted, ClientError>;

    /// Replaces `lease_id`'s egress `network_policy` in place.
    async fn update_network_policy(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        network_policy: NetworkPolicy,
    ) -> Result<(), ClientError>;

    async fn create_drive(
        &self,
        api_key: &str,
        tenant: TenantId,
        drive_id: DriveId,
        size_gib: Option<u64>,
        storage: DriveStorage,
    ) -> Result<DriveRecord, ClientError>;

    async fn list_drives(&self, api_key: &str, tenant: TenantId) -> Result<Vec<DriveRecord>, ClientError>;

    async fn delete_drive(&self, api_key: &str, tenant: TenantId, drive_id: DriveId) -> Result<(), ClientError>;

    async fn run_command(&self, api_key: &str, request: RunCommandRequest) -> Result<RunCommandOutcome, ClientError>;

    /// Lists the named or probed processes started on `lease_id`.
    async fn list_processes(&self, api_key: &str, lease_id: &LeaseId) -> Result<ListProcessesResponse, ClientError>;

    /// Waits for the readiness probe of a process started with a spec.
    async fn wait_ready(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        timeout_seconds: u32,
    ) -> Result<WaitReadyResponse, ClientError>;

    /// Waits until the sandbox is running and `ports` accept connections.
    async fn wait_sandbox_ready(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        ports: ReadyPorts,
        timeout_seconds: u32,
    ) -> Result<WaitSandboxReadyResponse, ClientError>;

    /// Sends `signal` to the running process named `name` on `lease_id`.
    async fn kill_process(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        name: &ProcessName,
        signal: UnixSignal,
    ) -> Result<KillCommandResponse, ClientError>;

    async fn poll_command(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
    ) -> Result<PollCommandResponse, ClientError>;

    async fn cancel_command_launch(
        &self,
        _api_key: &str,
        _request: CancelCommandLaunchRequest,
    ) -> Result<CancelCommandLaunchResponse, ClientError> {
        Ok(CancelCommandLaunchResponse::Unsupported)
    }

    /// Sends `signal` to a previously started detached command on `lease_id`.
    async fn kill_command(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        signal: UnixSignal,
    ) -> Result<KillCommandResponse, ClientError>;

    /// Gracefully stops a detached command (SIGTERM, grace, SIGKILL).
    async fn stop_command(
        &self,
        api_key: &str,
        request: StopCommandRequest,
    ) -> Result<StopCommandResponse, ClientError>;

    /// Stops a detached command and starts it again under a new execution id.
    async fn restart_command(
        &self,
        api_key: &str,
        request: RestartCommandRequest,
    ) -> Result<RestartCommandResponse, ClientError>;

    /// Reads one page of durable command output after a cursor.
    async fn read_logs(&self, api_key: &str, request: ReadLogsRequest) -> Result<ReadLogsResponse, ClientError>;

    /// Writes `data` (and closes stdin when `eof`) to an interactive command on `lease_id`.
    async fn write_stdin(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        seq: StdinSeq,
        data: StdinChunk,
        eof: StdinEof,
    ) -> Result<WriteStdinResponse, ClientError>;

    /// Sends one terminal operation for `terminal_id` on `lease_id`.
    async fn terminal(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        terminal_id: TerminalId,
        op: TerminalOp,
    ) -> Result<TerminalResponse, ClientError>;

    /// Starts forwarding `terminal_id`'s output frames to `tx` (stream id = the
    /// terminal id) until `stop` is cancelled or the terminal exits. The
    /// subscription is live before this returns, so a terminal opened right
    /// after cannot lose its first prompt.
    async fn subscribe_terminal(
        &self,
        terminal_id: TerminalId,
        tx: EventSender,
        stop: CancellationToken,
    ) -> Result<(), BackendError>;

    async fn write_files(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        files: Vec<SandboxFileWrite>,
        as_user: Option<SandboxUserName>,
    ) -> Result<(), ClientError>;

    async fn read_file(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        path: String,
        as_user: Option<SandboxUserName>,
    ) -> Result<Vec<u8>, ClientError>;

    /// Runs one structured file operation (list, stat, ranged read, edit, search, remove,
    /// mkdir, rename, archive). Typed refusals come back as the matching [`ClientError`].
    async fn file_op(&self, api_key: &str, request: FileOpRequest) -> Result<FileOpResponse, ClientError>;

    async fn create_user(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        username: SandboxUserName,
    ) -> Result<CreateSandboxUserResponse, ClientError>;

    async fn create_group(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        groupname: SandboxGroupName,
    ) -> Result<CreateSandboxGroupResponse, ClientError>;

    async fn modify_group_membership(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        username: SandboxUserName,
        groupname: SandboxGroupName,
        action: GroupMembershipAction,
    ) -> Result<(), ClientError>;

    /// Resolves `port` from `lease_id`'s exposed ports into a reachable
    /// endpoint. The endpoint is the address the preview proxy and clients connect to.
    async fn get_sandbox_domain(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        port: u16,
    ) -> Result<GetSandboxDomainResponse, ClientError>;

    /// The vendor's own sandbox handle for `lease_id`.
    async fn native_handle(&self, api_key: &str, lease_id: &LeaseId) -> Result<NativeHandleResponse, ClientError>;

    /// Direct SSH access to `lease_id`'s sandbox, minted by the provider.
    async fn ssh_access(
        &self,
        api_key: &str,
        lease_id: &LeaseId,
        ttl_seconds: Option<u32>,
    ) -> Result<SshAccessResponse, ClientError>;

    /// Reads one persistent sandbox entity by id or name.
    async fn get_entity(
        &self,
        api_key: &str,
        tenant: TenantId,
        entity: EntityRef,
    ) -> Result<GetEntityResponse, ClientError>;

    /// Lists the tenant's persistent sandbox entities, one page at a time.
    async fn list_entities(
        &self,
        api_key: &str,
        request: ListEntitiesRequest,
    ) -> Result<ListEntitiesResponse, ClientError>;

    /// Permanently deletes a persistent sandbox entity by id or name.
    async fn delete_entity(
        &self,
        api_key: &str,
        tenant: TenantId,
        entity: EntityRef,
    ) -> Result<DeleteEntityResponse, ClientError>;

    /// Looks up a running lease's row by its tenant-scoped `name`.
    async fn get_lease_by_name(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: LeaseName,
    ) -> Result<GetLeaseByNameResponse, ClientError>;

    /// Permanently deletes a sandbox by its tenant-scoped `name`, including
    /// one with no active lease.
    async fn delete_named_sandbox(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: LeaseName,
    ) -> Result<DeleteNamedSandboxResponse, ClientError>;

    /// Looks up a single snapshot by digest.
    async fn snapshot_get(
        &self,
        api_key: &str,
        tenant: TenantId,
        digest: Digest,
    ) -> Result<SnapshotGetResponse, ClientError>;

    /// Walks a snapshot digest's lineage tree.
    async fn snapshot_tree(
        &self,
        api_key: &str,
        tenant: TenantId,
        digest: Digest,
    ) -> Result<SnapshotTreeResponse, ClientError>;

    /// Points a named snapshot tag at `digest`.
    async fn set_snapshot_tag(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: SnapshotTagName,
        digest: Digest,
    ) -> Result<(), ClientError>;

    /// Removes a named snapshot tag.
    async fn delete_snapshot_tag(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: SnapshotTagName,
    ) -> Result<DeleteSnapshotTagResponse, ClientError>;

    /// Lists the history of digests a named snapshot tag has pointed at.
    async fn snapshot_tag_history(
        &self,
        api_key: &str,
        tenant: TenantId,
        name: SnapshotTagName,
    ) -> Result<SnapshotTagHistoryResponse, ClientError>;

    async fn list_capabilities(&self) -> CapabilitiesResponse;

    /// Starts following a command's output from `request.after_seq`: the
    /// retained chunks, then live ones, as events on
    /// [`crate::follow_logs_stream_id`], ending with an `end` event once the command
    /// exits. Returns once the follow is running; it stops when `tx` closes.
    /// The caller's tenant is enforced by the broker on every read.
    async fn follow_logs(&self, api_key: String, request: ReadLogsRequest, tx: EventSender)
    -> Result<(), BackendError>;

    /// Completes the evidence subscription before returning, so the caller can
    /// tell the client it is subscribed only once events cannot be missed.
    async fn subscribe_evidence(&self, api_key: String, lease_id: LeaseId, tx: EventSender)
    -> Result<(), BackendError>;

    async fn list_leases(
        &self,
        tenant: &TenantId,
        tag_filter: &BTreeMap<String, String>,
    ) -> Result<Value, BackendError> {
        let _ = (tenant, tag_filter);
        Ok(empty_listing("leases"))
    }

    /// Releases the session tenant's leases matching the request's filters
    /// (or reports them on a dry run).
    async fn reap_leases(&self, api_key: &str, request: ReapLeasesRequest) -> Result<ReapLeasesResponse, ClientError>;

    async fn list_snapshots(&self, tenant: &TenantId) -> Result<Value, BackendError> {
        let _ = tenant;
        Ok(empty_listing("snapshots"))
    }

    async fn list_tags(&self, tenant: &TenantId) -> Result<Value, BackendError> {
        let _ = tenant;
        Ok(empty_listing("tags"))
    }

    fn spawn_dashboard_watchers(&self, _tenant: TenantId, _tx: EventSender, _cancel: CancellationToken) {}
}
