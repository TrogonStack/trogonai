use trogon_sandbox_contract::{
    CredentialProvider, Digest, ExecutionId, LeaseId, ProviderKind, SnapshotTagName, TenantId, TerminalId, WorkerId,
};

pub use crate::constants::{
    CAPABILITY_LIST_REQUEST, CREDENTIAL_AUDIT_STREAM, CREDENTIAL_TOKEN_BUCKET, DELETE_NAMED_SANDBOX_REQUEST,
    DRIVE_CREATE_REQUEST, DRIVE_DELETE_REQUEST, DRIVE_KV_BUCKET, DRIVE_LIST_REQUEST, EGRESS_DECIDE_FILTER,
    EGRESS_REQUEST_DECIDE_FILTER, EGRESS_VIOLATION_FILTER, ENTITY_DELETE_REQUEST, ENTITY_EVENT_STREAM,
    ENTITY_EVENT_SUBJECT_PREFIX, ENTITY_GET_REQUEST, ENTITY_KV_BUCKET, ENTITY_LIST_REQUEST,
    ENTITY_NAME_INDEX_KV_BUCKET, FS_VIOLATION_FILTER, LEASE_CANCEL_COMMAND_LAUNCH_FILTER, LEASE_CANCEL_FILTER,
    LEASE_CREATE_GROUP_FILTER, LEASE_CREATE_USER_FILTER, LEASE_DELETE_FILTER, LEASE_EXTEND_TIMEOUT_FILTER,
    LEASE_FILE_OP_FILTER, LEASE_FORK_REQUEST, LEASE_GET_BY_NAME_REQUEST, LEASE_GET_DOMAIN_FILTER,
    LEASE_KILL_COMMAND_FILTER, LEASE_LIST_PROCESSES_FILTER, LEASE_LIST_REQUEST, LEASE_MODIFY_GROUP_MEMBERSHIP_FILTER,
    LEASE_NAME_INDEX_KV_BUCKET, LEASE_NATIVE_HANDLE_FILTER, LEASE_NETWORK_POLICY_UPDATE_FILTER, LEASE_PAUSE_FILTER,
    LEASE_PENDING_PURGE_KV_BUCKET, LEASE_PENDING_RELEASE_KV_BUCKET, LEASE_POLL_COMMAND_FILTER, LEASE_READ_FILE_FILTER,
    LEASE_READ_LOGS_FILTER, LEASE_REAP_REQUEST, LEASE_RELEASED_FILTER, LEASE_REQUEST, LEASE_RESOLVED_DIGEST_KV_BUCKET,
    LEASE_RESTART_COMMAND_FILTER, LEASE_RESUME_FILTER, LEASE_ROLLBACK_FILTER, LEASE_RUN_COMMAND_FILTER,
    LEASE_SNAPSHOT_FILTER, LEASE_SSH_ACCESS_FILTER, LEASE_STOP_COMMAND_FILTER, LEASE_STOP_FILTER, LEASE_SUSPEND_FILTER,
    LEASE_TAGS_UPDATE_FILTER, LEASE_TERMINAL_FILTER, LEASE_USAGE_FILTER, LEASE_USAGE_KV_BUCKET,
    LEASE_WAIT_READY_FILTER, LEASE_WAIT_SANDBOX_READY_FILTER, LEASE_WAKE_FILTER, LEASE_WRITE_FILES_FILTER,
    LEASE_WRITE_STDIN_FILTER, NATIVE_SNAPSHOT_DELETE_REQUEST, NATIVE_SNAPSHOT_KV_BUCKET, OPERATION_CANCEL_REQUEST,
    OPERATION_CAPABILITIES_REQUEST, OPERATION_GET_REQUEST, PREWARM_KV_BUCKET, SNAPSHOT_ACCOUNTING_REQUEST,
    SNAPSHOT_ARTIFACT_KV_BUCKET, SNAPSHOT_CAPTURE_STATUS_KV_BUCKET, SNAPSHOT_DELETE_REQUEST, SNAPSHOT_DELETED,
    SNAPSHOT_GET_REQUEST, SNAPSHOT_KV_BUCKET, SNAPSHOT_LAST_USED_KV_BUCKET, SNAPSHOT_LIVE_KV_BUCKET,
    SNAPSHOT_PUBLICATION_RECEIPT_KV_BUCKET, SNAPSHOT_PUBLISHED, SNAPSHOT_TAG_AUDIT_STREAM, SNAPSHOT_TAG_DELETE_REQUEST,
    SNAPSHOT_TAG_HISTORY_REQUEST, SNAPSHOT_TAG_KV_BUCKET, SNAPSHOT_TAG_SET_REQUEST, SNAPSHOT_TREE_REQUEST,
    TENANT_LIMITS_REQUEST, TENANT_USAGE_REQUEST, WORKER_AUTHORITY_FILTER, WORKER_LEASE_ACTIVITY_FILTER,
};
use crate::kv_key::{legacy_tenant_scoped_kv_key, tenant_scoped_kv_key};
use crate::unknown_provider_kind_error::UnknownProviderKindError;

pub fn worker_dispatch(provider: ProviderKind, worker: &WorkerId) -> String {
    format!(
        "sandbox.provider.{}.worker.{}.lease",
        provider_token(provider),
        worker.as_str()
    )
}

pub fn worker_cleanup(provider: ProviderKind, worker: &WorkerId) -> String {
    format!(
        "sandbox.provider.{}.worker.{}.cleanup",
        provider_token(provider),
        worker.as_str()
    )
}

pub fn worker_command_readiness(provider: ProviderKind, worker: &WorkerId, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.worker.{}.command_readiness.{}",
        provider_token(provider),
        worker.as_str(),
        lease.as_str(),
    )
}

pub fn worker_command_readiness_filter(provider: ProviderKind, worker: &WorkerId) -> String {
    format!(
        "sandbox.provider.{}.worker.{}.command_readiness.>",
        provider_token(provider),
        worker.as_str(),
    )
}

pub fn lease_snapshot_in(lease: &LeaseId) -> String {
    format!("sandbox.broker.kv.lease.{}.snapshot_in", lease.as_str())
}

pub fn live_snapshot_kv(digest: &Digest) -> String {
    format!("sandbox.broker.kv.snapshot.{}", digest.as_str())
}

pub fn lease_failed(lease: &LeaseId) -> String {
    format!("sandbox.lease.failed.{}", lease.as_str())
}

pub fn snapshot_result_evidence(lease: &LeaseId) -> String {
    format!("sandbox.evidence.{}.snapshot-result", lease.as_str())
}

/// Evidence subject for a file watch that dropped events (see
/// [`trogon_sandbox_contract::FileWatchOverflowEvidence`]).
pub fn file_watch_overflow_evidence(lease: &LeaseId) -> String {
    format!("sandbox.evidence.{}.file-watch-overflow", lease.as_str())
}

pub fn snapshot_result_evidence_filter() -> &'static str {
    "sandbox.evidence.*.snapshot-result"
}

pub fn provider_token(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::K8sMicroVm => "k8s_micro_vm",
        ProviderKind::Subprocess => "subprocess",
        ProviderKind::LocalDocker => "local_docker",
        ProviderKind::Daytona => "daytona",
        ProviderKind::Modal => "modal",
        ProviderKind::Hyperlight => "hyperlight",
        ProviderKind::V8Wasm => "v8_wasm",
        ProviderKind::KubeVirt => "kubevirt",
        ProviderKind::AzureContainerApps => "azure_container_apps",
        ProviderKind::Substrate => "substrate",
        ProviderKind::Firecracker => "firecracker",
        ProviderKind::Proxmox => "proxmox",
        ProviderKind::Fly => "fly",
        ProviderKind::E2b => "e2b",
        ProviderKind::Vercel => "vercel",
        ProviderKind::Space => "space",
        ProviderKind::Hluk => "hluk",
        ProviderKind::Mock => "mock",
    }
}

pub fn lease_cancel(lease: &LeaseId) -> String {
    format!("sandbox.lease.cancel.{}", lease.as_str())
}

/// Client-facing request to fully replace a running lease's `tags` set.
pub fn lease_tags_update(lease: &LeaseId) -> String {
    format!("sandbox.lease.tags.update.{}", lease.as_str())
}

/// Client-facing request to replace the egress `network_policy` of an
/// already-running lease in place (see
/// [`trogon_sandbox_contract::NetworkPolicyUpdateRequest`]).
pub fn lease_network_policy_update(lease: &LeaseId) -> String {
    format!("sandbox.lease.network_policy_update.{}", lease.as_str())
}

pub fn provider_cancel_owned(provider: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.cancel_owned.{}",
        provider_token(provider),
        lease.as_str()
    )
}

pub fn provider_native_snapshot_delete(provider: ProviderKind) -> String {
    format!("sandbox.provider.{}.native_snapshot_delete", provider_token(provider))
}

/// Client-facing request to extend a running lease's wall-clock deadline.
pub fn lease_extend_timeout(lease: &LeaseId) -> String {
    format!("sandbox.lease.extend_timeout.{}", lease.as_str())
}

/// Client-facing request to permanently delete a lease (distinct from cancel;
/// see [`trogon_sandbox_contract::DeleteLeaseRequest`]).
pub fn lease_delete(lease: &LeaseId) -> String {
    format!("sandbox.lease.delete.{}", lease.as_str())
}

/// Client-facing request to stop a lease while keeping it resumable (distinct
/// from cancel and delete; see [`trogon_sandbox_contract::StopLeaseRequest`]).
pub fn lease_stop(lease: &LeaseId) -> String {
    format!("sandbox.lease.stop.{}", lease.as_str())
}

/// Client-facing request to pause a lease while keeping its state and id (see
/// [`trogon_sandbox_contract::PauseLeaseRequest`]).
pub fn lease_pause(lease: &LeaseId) -> String {
    format!("sandbox.lease.pause.{}", lease.as_str())
}

/// Client-facing request to snapshot a running lease (see
/// [`trogon_sandbox_contract::SnapshotLeaseRequest`]).
pub fn lease_snapshot(lease: &LeaseId) -> String {
    format!("sandbox.lease.snapshot.{}", lease.as_str())
}

/// Client-facing request to resume a paused lease (see
/// [`trogon_sandbox_contract::ResumeLeaseRequest`]).
pub fn lease_resume(lease: &LeaseId) -> String {
    format!("sandbox.lease.resume.{}", lease.as_str())
}

/// Client-facing request to suspend a lease: release its host resources and
/// keep a durable artifact (see [`trogon_sandbox_contract::SuspendLeaseRequest`]).
pub fn lease_suspend(lease: &LeaseId) -> String {
    format!("sandbox.lease.suspend.{}", lease.as_str())
}

/// Client-facing request to reset a lease to a snapshot in place (see
/// [`trogon_sandbox_contract::RollbackLeaseRequest`]).
pub fn lease_rollback(lease: &LeaseId) -> String {
    format!("sandbox.lease.rollback.{}", lease.as_str())
}

/// Client-facing request to wake a suspended lease (see
/// [`trogon_sandbox_contract::WakeLeaseRequest`]).
pub fn lease_wake(lease: &LeaseId) -> String {
    format!("sandbox.lease.wake.{}", lease.as_str())
}

/// Client-facing request for one live lease's usage counters and gauges (see
/// [`trogon_sandbox_contract::GetLeaseUsageRequest`]).
pub fn lease_usage(lease: &LeaseId) -> String {
    format!("sandbox.lease.usage.{}", lease.as_str())
}

/// Client-facing request to run an arbitrary command inside an
/// already-running lease's sandbox (see [`trogon_sandbox_contract::RunCommandRequest`]).
pub fn lease_run_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.run_command.{}", lease.as_str())
}

/// Client-facing request to send a Unix signal to a previously started
/// command (see [`trogon_sandbox_contract::KillCommandRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_kill_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.kill_command.{}", lease.as_str())
}

pub fn lease_cancel_command_launch(lease: &LeaseId) -> String {
    format!("sandbox.lease.cancel_command_launch.{}", lease.as_str())
}

/// Client-facing request to stop a previously started command gracefully
/// (see [`trogon_sandbox_contract::StopCommandRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_stop_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.stop_command.{}", lease.as_str())
}

/// Client-facing request to run a previously started command again (see
/// [`trogon_sandbox_contract::RestartCommandRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_restart_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.restart_command.{}", lease.as_str())
}

/// Client-facing request to read a command's retained output in pages (see
/// [`trogon_sandbox_contract::ReadLogsRequest`]). The broker answers it from the command
/// log stream, so it is not forwarded to a worker.
pub fn lease_read_logs(lease: &LeaseId) -> String {
    format!("sandbox.lease.read_logs.{}", lease.as_str())
}

/// Client-facing request to write to the stdin of a previously started
/// interactive command (see [`trogon_sandbox_contract::WriteStdinRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_write_stdin(lease: &LeaseId) -> String {
    format!("sandbox.lease.write_stdin.{}", lease.as_str())
}

/// Client-facing request to open, feed, resize or close an interactive
/// terminal (see [`trogon_sandbox_contract::TerminalRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_terminal(lease: &LeaseId) -> String {
    format!("sandbox.lease.terminal.{}", lease.as_str())
}

/// Core NATS subject a worker publishes a terminal's
/// [`trogon_sandbox_contract::TerminalOutput`] frames on. Keyed by terminal id alone so
/// a reader can subscribe before the open is sent.
pub fn terminal_output(terminal_id: &TerminalId) -> String {
    format!("sandbox.terminal.out.{}", terminal_id.as_str())
}

/// Core NATS subject a worker publishes on (empty payload) when a lease shows
/// activity the broker cannot see, such as terminal output. The broker counts
/// it toward the lease's idle timeout. Workers throttle it.
pub fn worker_lease_activity(lease: &LeaseId) -> String {
    format!("sandbox.worker.activity.{}", lease.as_str())
}

/// The lease a [`worker_lease_activity`] subject names, or `None` when the
/// subject is not one.
pub fn parse_worker_lease_activity(subject: &str) -> Option<LeaseId> {
    subject
        .strip_prefix("sandbox.worker.activity.")
        .filter(|lease| !lease.is_empty() && !lease.contains('.'))
        .map(LeaseId::new)
}

/// Request/reply a worker sends the broker to learn which of the leases it
/// holds are still assigned to it (see
/// [`trogon_sandbox_contract::WorkerAuthorityRequest`]). Shaped
/// `sandbox.worker.authority.{}` so a worker can be limited to its own
/// subject and the broker can answer every worker with
/// [`WORKER_AUTHORITY_FILTER`].
pub fn worker_authority_check(worker: &WorkerId) -> String {
    format!("sandbox.worker.authority.{}", worker.as_str())
}

/// The worker a [`worker_authority_check`] subject names, or `None` when the
/// subject is not one.
pub fn parse_worker_authority_check(subject: &str) -> Option<WorkerId> {
    subject
        .strip_prefix("sandbox.worker.authority.")
        .and_then(|worker| WorkerId::new(worker).ok())
}

/// Client-facing request to resolve a port from
/// [`trogon_sandbox_contract::SandboxSpec::exposed_ports`] on an already-running lease
/// into a reachable endpoint (see [`trogon_sandbox_contract::GetSandboxDomainRequest`]).
pub fn lease_get_domain(lease: &LeaseId) -> String {
    format!("sandbox.lease.get_domain.{}", lease.as_str())
}

/// Client-facing request for the vendor's own handle on a lease (see
/// [`trogon_sandbox_contract::NativeHandleRequest`]).
pub fn lease_native_handle(lease: &LeaseId) -> String {
    format!("sandbox.lease.native_handle.{}", lease.as_str())
}

/// Client-facing request for direct SSH access to a lease (see
/// [`trogon_sandbox_contract::SshAccessRequest`]).
pub fn lease_ssh_access(lease: &LeaseId) -> String {
    format!("sandbox.lease.ssh_access.{}", lease.as_str())
}

/// Client-facing request to poll the status of a previously started detached
/// command (see [`trogon_sandbox_contract::PollCommandRequest`]).
pub fn lease_poll_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.poll_command.{}", lease.as_str())
}

/// Client-facing request to list the named or probed processes of a lease
/// (see [`trogon_sandbox_contract::ListProcessesRequest`]).
pub fn lease_list_processes(lease: &LeaseId) -> String {
    format!("sandbox.lease.list_processes.{}", lease.as_str())
}

/// Client-facing request to wait for a process's readiness probe (see
/// [`trogon_sandbox_contract::WaitReadyRequest`]).
pub fn lease_wait_ready(lease: &LeaseId) -> String {
    format!("sandbox.lease.wait_ready.{}", lease.as_str())
}

/// Client-facing request to wait until a sandbox is usable (see
/// [`trogon_sandbox_contract::WaitSandboxReadyRequest`]).
pub fn lease_wait_sandbox_ready(lease: &LeaseId) -> String {
    format!("sandbox.lease.wait_sandbox_ready.{}", lease.as_str())
}

/// Client-facing request to write one or more files into an already-running
/// lease's sandbox (see [`trogon_sandbox_contract::WriteFilesRequest`]).
pub fn lease_write_files(lease: &LeaseId) -> String {
    format!("sandbox.lease.write_files.{}", lease.as_str())
}

/// Client-facing request to read a single file out of an already-running
/// lease's sandbox (see [`trogon_sandbox_contract::ReadFileRequest`]).
pub fn lease_read_file(lease: &LeaseId) -> String {
    format!("sandbox.lease.read_file.{}", lease.as_str())
}

/// Client-facing request for one structured file operation inside an
/// already-running lease's sandbox (see [`trogon_sandbox_contract::FileOpRequest`]).
pub fn lease_file_op(lease: &LeaseId) -> String {
    format!("sandbox.lease.file_op.{}", lease.as_str())
}

/// Client-facing request to create a per-sandbox Linux user inside an
/// already-running lease's sandbox (see [`trogon_sandbox_contract::CreateSandboxUserRequest`]).
pub fn lease_create_user(lease: &LeaseId) -> String {
    format!("sandbox.lease.create_user.{}", lease.as_str())
}

/// Client-facing request to create a per-sandbox Linux group inside an
/// already-running lease's sandbox (see [`trogon_sandbox_contract::CreateSandboxGroupRequest`]).
pub fn lease_create_group(lease: &LeaseId) -> String {
    format!("sandbox.lease.create_group.{}", lease.as_str())
}

/// Client-facing request to add or remove a sandbox user's group membership
/// inside an already-running lease's sandbox (see
/// [`trogon_sandbox_contract::ModifyGroupMembershipRequest`]).
pub fn lease_modify_group_membership(lease: &LeaseId) -> String {
    format!("sandbox.lease.modify_group_membership.{}", lease.as_str())
}

/// Request/reply asked by a lease's egress proxy about a destination its
/// policy neither allows nor denies (see [`trogon_sandbox_contract::DecideRequest`]).
/// Shaped `sandbox.egress.decide.{lease_id}` so one decider service can answer
/// every lease with [`EGRESS_DECIDE_FILTER`].
pub fn egress_decide(lease: &LeaseId) -> String {
    format!("sandbox.egress.decide.{}", lease.as_str())
}

/// Request/reply asked by a lease's egress proxy about one HTTP request to an
/// admitted destination (see [`trogon_sandbox_contract::RequestDecideRequest`]). Shaped
/// `sandbox.egress.request.{lease_id}`.
pub fn egress_request_decide(lease: &LeaseId) -> String {
    format!("sandbox.egress.request.{}", lease.as_str())
}

/// Published by a lease's egress proxy once per denied connection, subject to
/// per-lease rate limiting (see [`trogon_sandbox_contract::EgressViolation`]). Shaped
/// `sandbox.egress.violation.{lease_id}` so alerting and audit subscribers can
/// take every lease with [`EGRESS_VIOLATION_FILTER`].
pub fn egress_violation(lease: &LeaseId) -> String {
    format!("sandbox.egress.violation.{}", lease.as_str())
}

/// Published by a lease's worker once per refused filesystem operation, subject
/// to per-lease rate limiting (see [`trogon_sandbox_contract::FsViolation`]). Shaped
/// `sandbox.fs.violation.{lease_id}`, alongside [`egress_violation`].
pub fn fs_violation(lease: &LeaseId) -> String {
    format!("sandbox.fs.violation.{}", lease.as_str())
}

pub fn lease_released(lease: &LeaseId) -> String {
    format!("sandbox.lease.released.{}", lease.as_str())
}

pub fn worker_snapshot_export(worker_id: &WorkerId) -> String {
    format!("sandbox.worker.{}.snapshot.export", worker_id.as_str())
}

pub fn snapshot_kv_key(digest: &Digest) -> String {
    digest.as_str().replace(':', "_")
}

pub fn prewarm_kv_key(tenant: &TenantId, digest: &Digest) -> String {
    tenant_scoped_kv_key(tenant, &snapshot_kv_key(digest))
}

/// Pre-injective prewarm key, kept only to find records written before
/// [`prewarm_kv_key`] escaped the tenant.
pub fn legacy_prewarm_kv_key(tenant: &TenantId, digest: &Digest) -> String {
    legacy_tenant_scoped_kv_key(tenant, &snapshot_kv_key(digest))
}

pub fn provider_dispatch(kind: ProviderKind) -> String {
    format!("sandbox.provider.{}.lease", provider_token(kind))
}

pub fn provider_cancel(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.cancel.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject for a live checkpoint of a still-running lease. Every
/// worker for `kind` subscribes to the wildcard form of this subject and only
/// the one actually holding `lease` replies, mirroring [`provider_cancel`].
pub fn provider_checkpoint_running(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.checkpoint_running.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for applying a network policy update to an already-running
/// lease. Every worker for `kind` subscribes to the wildcard form of this
/// subject and only the one actually holding `lease` replies, mirroring
/// [`provider_checkpoint_running`].
pub fn provider_network_policy_update(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.network_policy_update.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for running an arbitrary command inside an
/// already-running lease's sandbox. Every worker for `kind` subscribes to the
/// wildcard form of this subject and only the one actually holding `lease`
/// replies, mirroring [`provider_checkpoint_running`].
pub fn provider_run_command(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.run_command.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for resolving a port into a reachable endpoint on an
/// already-running lease's sandbox. Routed like [`provider_run_command`].
pub fn provider_get_domain(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.get_domain.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for reading the vendor's handle on a lease's sandbox.
/// Routed like [`provider_run_command`].
pub fn provider_native_handle(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.native_handle.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for minting provider-side SSH access to a lease's
/// sandbox. Routed like [`provider_run_command`].
pub fn provider_ssh_access(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.ssh_access.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for freezing a lease's sandbox in place. Routed like
/// [`provider_run_command`].
pub fn provider_pause(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.pause.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject for thawing a paused lease's sandbox. Routed like
/// [`provider_run_command`].
pub fn provider_resume(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.resume.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject for rebuilding a lease's sandbox from a snapshot (see
/// [`trogon_sandbox_contract::SandboxRollbackRequest`]). Routed like [`provider_run_command`].
pub fn provider_rollback(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.rollback.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject for suspending a lease's sandbox (see
/// [`trogon_sandbox_contract::SandboxSuspendRequest`]). Routed like [`provider_run_command`].
pub fn provider_suspend(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.suspend.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject for waking a suspended lease's sandbox (see
/// [`trogon_sandbox_contract::SandboxWakeRequest`]). Routed like [`provider_run_command`].
pub fn provider_wake(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.wake.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject asking the worker holding a lease for its live resource
/// gauges. Routed like [`provider_run_command`].
pub fn provider_resource_usage(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.resource_usage.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for sending a Unix signal to a previously started
/// command inside an already-running lease's sandbox. Routed like
/// [`provider_run_command`].
pub fn provider_kill_command(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.kill_command.{}",
        provider_token(kind),
        lease.as_str()
    )
}

pub fn provider_cancel_command_launch(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.cancel_command_launch.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for stopping a previously started command gracefully.
/// Routed like [`provider_run_command`].
pub fn provider_stop_command(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.stop_command.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for running a previously started command again. Routed
/// like [`provider_run_command`].
pub fn provider_restart_command(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.restart_command.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for writing to the stdin of a previously started
/// interactive command. Routed like [`provider_run_command`].
pub fn provider_write_stdin(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.write_stdin.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for terminal operations. Routed like
/// [`provider_run_command`].
pub fn provider_terminal(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.terminal.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject for writing files into an already-running lease's
/// sandbox. Routed like [`provider_run_command`].
pub fn provider_write_files(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.write_files.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for reading a file out of an already-running lease's
/// sandbox. Routed like [`provider_run_command`].
pub fn provider_read_file(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.read_file.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject for a structured file operation inside an already-running
/// lease's sandbox. Routed like [`provider_run_command`].
pub fn provider_file_op(kind: ProviderKind, lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.file_op.{}", provider_token(kind), lease.as_str())
}

/// Broadcast subject for polling the status of a previously started detached
/// command inside an already-running lease's sandbox. Routed like
/// [`provider_run_command`].
pub fn provider_poll_command(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.poll_command.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for listing the named or probed processes of a lease.
/// Routed like [`provider_run_command`].
pub fn provider_list_processes(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.list_processes.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for waiting on a process's readiness probe. Routed like
/// [`provider_run_command`].
pub fn provider_wait_ready(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.wait_ready.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for creating a per-sandbox Linux user inside an
/// already-running lease's sandbox. Routed like [`provider_run_command`].
pub fn provider_create_user(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.create_user.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for creating a per-sandbox Linux group inside an
/// already-running lease's sandbox. Routed like [`provider_run_command`].
pub fn provider_create_group(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.create_group.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Broadcast subject for adding or removing a sandbox user's group membership
/// inside an already-running lease's sandbox. Routed like
/// [`provider_run_command`].
pub fn provider_modify_group_membership(kind: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.modify_group_membership.{}",
        provider_token(kind),
        lease.as_str()
    )
}

/// Fixed (not lease-scoped) subject for purging a named sandbox's suspended
/// actor by tenant + name, used by the broker's
/// [`trogon_sandbox_contract::DeleteNamedSandboxRequest`] handler, which must reach the
/// sandbox even when no lease holds it any more. Any worker for `kind` can
/// serve this since actors are cluster-wide, so callers should subscribe with
/// a NATS queue group rather than broadcasting like [`provider_cancel`].
pub fn provider_purge_named(kind: ProviderKind) -> String {
    format!("sandbox.provider.{}.purge_named", provider_token(kind))
}

pub fn evidence_subject(lease: &LeaseId) -> String {
    format!("sandbox.evidence.{}.>", lease.as_str())
}

/// Suffix after `sandbox.evidence.{lease_id}.` (e.g. `lifecycle`, `review-result`).
pub fn evidence_suffix(lease: &LeaseId, full_subject: &str) -> Option<String> {
    let prefix = format!("sandbox.evidence.{}.", lease.as_str());
    full_subject
        .strip_prefix(&prefix)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Suffix for a streamed command's output chunks (see
/// [`trogon_sandbox_contract::RunCommandRequest::stream_output`]), published under the
/// existing evidence subject `sandbox.evidence.{lease_id}.command.{execution_id}`.
pub fn evidence_command_suffix(execution_id: &ExecutionId) -> String {
    format!("command.{}", execution_id.as_str())
}

/// Suffix for a streamed command's exit marker (see
/// [`trogon_sandbox_contract::CommandExitMarker`]): `command_exit.{execution_id}`. A
/// separate token from `command` keeps chunk consumers filtering on
/// `command.*` from seeing a payload that is not a chunk.
pub fn evidence_command_exit_suffix(execution_id: &ExecutionId) -> String {
    format!("command_exit.{}", execution_id.as_str())
}

/// Full subject of a streamed command's exit marker on `lease_id`.
pub fn command_exit_subject(lease_id: &LeaseId, execution_id: &ExecutionId) -> String {
    format!(
        "sandbox.evidence.{}.{}",
        lease_id.as_str(),
        evidence_command_exit_suffix(execution_id)
    )
}

pub fn worker_heartbeat(provider: ProviderKind, worker_id: &WorkerId) -> String {
    format!(
        "sandbox.worker.heartbeat.{}.{}",
        provider_token(provider),
        worker_id.as_str()
    )
}

pub fn worker_heartbeat_filter(provider: ProviderKind) -> String {
    format!("sandbox.worker.heartbeat.{}.*", provider_token(provider))
}

pub fn parse_provider_kind(s: &str) -> Result<ProviderKind, UnknownProviderKindError> {
    match s {
        "k8s_micro_vm" | "k8s-micro-vm" | "k8s" => Ok(ProviderKind::K8sMicroVm),
        "subprocess" => Ok(ProviderKind::Subprocess),
        "local_docker" | "local-docker" | "docker" => Ok(ProviderKind::LocalDocker),
        "daytona" | "daytona-io" => Ok(ProviderKind::Daytona),
        "modal" | "modal-com" | "modal-labs" => Ok(ProviderKind::Modal),
        "hyperlight" => Ok(ProviderKind::Hyperlight),
        "v8_wasm" | "v8-wasm" | "v8wasm" => Ok(ProviderKind::V8Wasm),
        "kubevirt" => Ok(ProviderKind::KubeVirt),
        "azure_container_apps" | "azure-container-apps" | "aca" => Ok(ProviderKind::AzureContainerApps),
        "substrate" | "agent-substrate" | "ate" => Ok(ProviderKind::Substrate),
        "firecracker" | "fc" => Ok(ProviderKind::Firecracker),
        "proxmox" | "pve" => Ok(ProviderKind::Proxmox),
        "fly" | "fly_io" | "fly-io" => Ok(ProviderKind::Fly),
        "e2b" | "e2b-dev" => Ok(ProviderKind::E2b),
        "vercel" | "vercel-sandbox" => Ok(ProviderKind::Vercel),
        "space" | "perplexity-space" => Ok(ProviderKind::Space),
        "hluk" => Ok(ProviderKind::Hluk),
        "mock" => Ok(ProviderKind::Mock),
        other => Err(UnknownProviderKindError::new(other)),
    }
}

pub fn credential_issue(provider: CredentialProvider, tenant: &TenantId) -> String {
    format!("sandbox.credentials.issue.{}.{}", provider.as_str(), tenant.as_str())
}

pub fn credential_revoke(provider: CredentialProvider, tenant: &TenantId) -> String {
    format!("sandbox.credentials.revoke.{}.{}", provider.as_str(), tenant.as_str())
}

pub fn credential_rotate(provider: CredentialProvider) -> String {
    format!("sandbox.credentials.rotate.{}", provider.as_str())
}

/// JetStream stream name for the credential audit events (`.` is reserved for
/// subjects, so the subject prefix is spelled with `_`).
pub fn credential_audit_stream_name() -> String {
    CREDENTIAL_AUDIT_STREAM.replace('.', "_")
}

pub fn snapshot_tag_audit_subject(tenant: &TenantId, name: &SnapshotTagName) -> String {
    format!("{SNAPSHOT_TAG_AUDIT_STREAM}.{}.{}", tenant.as_str(), name.as_str())
}

/// KV bucket names allow only letters, digits, `-`, `_` and `=`; a `.` is
/// rejected by the client, so the broker could not start with the dotted name.
pub fn credential_kv_bucket(provider: CredentialProvider) -> String {
    format!("sandbox_credentials_kv_{}", provider.as_str())
}
