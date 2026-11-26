use trogon_sandbox_contract::{
    CredentialProvider, Digest, ExecutionId, LeaseId, ProviderKind, SnapshotTagName, TenantId, TerminalId, WorkerId,
};

use crate::kv_key::{legacy_tenant_scoped_kv_key, tenant_scoped_kv_key};
use crate::unknown_provider_kind_error::UnknownProviderKindError;

pub const LEASE_REQUEST: &str = "sandbox.lease.request";

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

/// Client-facing request to fork a running lease into a new, independent one.
pub const LEASE_FORK_REQUEST: &str = "sandbox.lease.fork";

/// Wildcard subscription for all per-lease release notifications.
pub const LEASE_RELEASED_FILTER: &str = "sandbox.lease.released.>";

pub const SNAPSHOT_PUBLISHED: &str = "sandbox.snapshot.published";
pub const SNAPSHOT_DELETED: &str = "sandbox.snapshot.deleted";
pub const SNAPSHOT_ACCOUNTING_REQUEST: &str = "sandbox.snapshot.account.published";
pub const SNAPSHOT_LIVE_KV_BUCKET: &str = "sandbox-snapshot-live-v1";
pub const SNAPSHOT_ARTIFACT_KV_BUCKET: &str = "sandbox-snapshot-accounting-v1";
pub const SNAPSHOT_PUBLICATION_RECEIPT_KV_BUCKET: &str = "sandbox-snapshot-publication-v1";

/// JetStream KV bucket holding live snapshot digest records (logical namespace
/// `sandbox.broker.kv.snapshot.{digest}`).
pub const SNAPSHOT_KV_BUCKET: &str = "sandbox-broker-kv-snapshot";

/// JetStream KV bucket for tenant-scoped prewarm policy and controller state.
///
/// Logical path: `sandbox.broker.kv.prewarm.{tenant}.{digest}`.
pub const PREWARM_KV_BUCKET: &str = "sandbox-broker-kv-prewarm";

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
        ProviderKind::Mock => "mock",
    }
}

/// Broker subscription for every [`lease_cancel`] subject.
pub const LEASE_CANCEL_FILTER: &str = "sandbox.lease.cancel.*";

pub fn lease_cancel(lease: &LeaseId) -> String {
    format!("sandbox.lease.cancel.{}", lease.as_str())
}

/// Client-facing request to fully replace a running lease's `tags` set.
pub fn lease_tags_update(lease: &LeaseId) -> String {
    format!("sandbox.lease.tags.update.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_tags_update`] requests.
pub const LEASE_TAGS_UPDATE_FILTER: &str = "sandbox.lease.tags.update.*";

/// Client-facing request to replace the egress `network_policy` of an
/// already-running lease in place (see
/// [`trogon_sandbox_contract::NetworkPolicyUpdateRequest`]).
pub fn lease_network_policy_update(lease: &LeaseId) -> String {
    format!("sandbox.lease.network_policy_update.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_network_policy_update`] requests.
pub const LEASE_NETWORK_POLICY_UPDATE_FILTER: &str = "sandbox.lease.network_policy_update.*";

/// Client-facing request/reply to look up one durable operation by id (see
/// [`trogon_sandbox_contract::GetOperationRequest`]). Tenant scoped through the
/// request body: another tenant's operation id answers `NotFound`.
pub const OPERATION_GET_REQUEST: &str = "sandbox.operation.get";
pub const OPERATION_CANCEL_REQUEST: &str = "sandbox.operation.cancel";
pub const OPERATION_CAPABILITIES_REQUEST: &str = "sandbox.operation.capabilities";

pub fn provider_cancel_owned(provider: ProviderKind, lease: &LeaseId) -> String {
    format!(
        "sandbox.provider.{}.cancel_owned.{}",
        provider_token(provider),
        lease.as_str()
    )
}

pub const DRIVE_CREATE_REQUEST: &str = "sandbox.drive.create";
pub const DRIVE_LIST_REQUEST: &str = "sandbox.drive.list";
pub const DRIVE_DELETE_REQUEST: &str = "sandbox.drive.delete";

/// Tenant-facing request to list provider capability profiles known to the
/// broker's worker registry (one profile per provider with a live heartbeat).
pub const CAPABILITY_LIST_REQUEST: &str = "sandbox.capability.list";

/// Client-facing request/reply to look up a single snapshot by digest (see
/// [`trogon_sandbox_contract::SnapshotGetRequest`]). Not scoped by lease id: snapshots are
/// tenant-owned, not lease-owned.
pub const SNAPSHOT_GET_REQUEST: &str = "sandbox.snapshot.get";

/// Client-facing request/reply to delete a snapshot by digest (see
/// [`trogon_sandbox_contract::SnapshotDeleteRequest`]).
pub const SNAPSHOT_DELETE_REQUEST: &str = "sandbox.snapshot.delete";

pub const NATIVE_SNAPSHOT_DELETE_REQUEST: &str = "sandbox.native_snapshot.delete";

pub fn provider_native_snapshot_delete(provider: ProviderKind) -> String {
    format!("sandbox.provider.{}.native_snapshot_delete", provider_token(provider))
}

/// Client-facing request/reply to walk a snapshot digest's lineage tree (see
/// [`trogon_sandbox_contract::SnapshotTreeRequest`]).
pub const SNAPSHOT_TREE_REQUEST: &str = "sandbox.snapshot.tree";

/// Client-facing request/reply to point a named snapshot tag at a digest (see
/// [`trogon_sandbox_contract::SetSnapshotTagRequest`]).
pub const SNAPSHOT_TAG_SET_REQUEST: &str = "sandbox.snapshot.tag.set";

/// Client-facing request/reply to remove a named snapshot tag (see
/// [`trogon_sandbox_contract::DeleteSnapshotTagRequest`]).
pub const SNAPSHOT_TAG_DELETE_REQUEST: &str = "sandbox.snapshot.tag.delete";

/// Client-facing request/reply to list a named snapshot tag's history (see
/// [`trogon_sandbox_contract::SnapshotTagHistoryRequest`]).
pub const SNAPSHOT_TAG_HISTORY_REQUEST: &str = "sandbox.snapshot.tag.history";

/// Client-facing request/reply to permanently delete a sandbox by its
/// tenant-scoped `name`, including when it has no active lease (see
/// [`trogon_sandbox_contract::DeleteNamedSandboxRequest`]). Not scoped by lease id: unlike
/// [`lease_delete`], this must still find the sandbox after its lease is gone.
pub const DELETE_NAMED_SANDBOX_REQUEST: &str = "sandbox.named.delete";

/// Client-facing request/reply to look up a lease by its tenant-scoped `name`
/// (see [`trogon_sandbox_contract::GetLeaseByNameRequest`]). Not scoped by lease id since
/// the lookup is by name.
pub const LEASE_GET_BY_NAME_REQUEST: &str = "sandbox.lease.get_by_name";

/// Client-facing request/reply to list a tenant's leases, paginated by an
/// opaque cursor (see [`trogon_sandbox_contract::ListLeasesRequest`]).
pub const LEASE_LIST_REQUEST: &str = "sandbox.lease.list";

/// Client-facing request/reply to release a tenant's leases by tag, age or
/// idleness, with a dry-run mode (see [`trogon_sandbox_contract::ReapLeasesRequest`]).
pub const LEASE_REAP_REQUEST: &str = "sandbox.lease.reap";

/// Client-facing request to extend a running lease's wall-clock deadline.
pub fn lease_extend_timeout(lease: &LeaseId) -> String {
    format!("sandbox.lease.extend_timeout.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_extend_timeout`] requests.
pub const LEASE_EXTEND_TIMEOUT_FILTER: &str = "sandbox.lease.extend_timeout.*";

/// Client-facing request to permanently delete a lease (distinct from cancel;
/// see [`trogon_sandbox_contract::DeleteLeaseRequest`]).
pub fn lease_delete(lease: &LeaseId) -> String {
    format!("sandbox.lease.delete.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_delete`] requests.
pub const LEASE_DELETE_FILTER: &str = "sandbox.lease.delete.*";

/// Client-facing request to stop a lease while keeping it resumable (distinct
/// from cancel and delete; see [`trogon_sandbox_contract::StopLeaseRequest`]).
pub fn lease_stop(lease: &LeaseId) -> String {
    format!("sandbox.lease.stop.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_stop`] requests.
pub const LEASE_STOP_FILTER: &str = "sandbox.lease.stop.*";

/// Client-facing request to pause a lease while keeping its state and id (see
/// [`trogon_sandbox_contract::PauseLeaseRequest`]).
pub fn lease_pause(lease: &LeaseId) -> String {
    format!("sandbox.lease.pause.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_pause`] requests.
pub const LEASE_PAUSE_FILTER: &str = "sandbox.lease.pause.*";

/// Client-facing request to snapshot a running lease (see
/// [`trogon_sandbox_contract::SnapshotLeaseRequest`]).
pub fn lease_snapshot(lease: &LeaseId) -> String {
    format!("sandbox.lease.snapshot.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_snapshot`] requests.
pub const LEASE_SNAPSHOT_FILTER: &str = "sandbox.lease.snapshot.*";

/// Client-facing request to resume a paused lease (see
/// [`trogon_sandbox_contract::ResumeLeaseRequest`]).
pub fn lease_resume(lease: &LeaseId) -> String {
    format!("sandbox.lease.resume.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_resume`] requests.
pub const LEASE_RESUME_FILTER: &str = "sandbox.lease.resume.*";

/// Client-facing request to suspend a lease: release its host resources and
/// keep a durable artifact (see [`trogon_sandbox_contract::SuspendLeaseRequest`]).
pub fn lease_suspend(lease: &LeaseId) -> String {
    format!("sandbox.lease.suspend.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_suspend`] requests.
pub const LEASE_SUSPEND_FILTER: &str = "sandbox.lease.suspend.*";

/// Client-facing request to reset a lease to a snapshot in place (see
/// [`trogon_sandbox_contract::RollbackLeaseRequest`]).
pub fn lease_rollback(lease: &LeaseId) -> String {
    format!("sandbox.lease.rollback.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_rollback`] requests.
pub const LEASE_ROLLBACK_FILTER: &str = "sandbox.lease.rollback.*";

/// Client-facing request to wake a suspended lease (see
/// [`trogon_sandbox_contract::WakeLeaseRequest`]).
pub fn lease_wake(lease: &LeaseId) -> String {
    format!("sandbox.lease.wake.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_wake`] requests.
pub const LEASE_WAKE_FILTER: &str = "sandbox.lease.wake.*";

/// Client-facing request for one live lease's usage counters and gauges (see
/// [`trogon_sandbox_contract::GetLeaseUsageRequest`]).
pub fn lease_usage(lease: &LeaseId) -> String {
    format!("sandbox.lease.usage.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_usage`] requests.
pub const LEASE_USAGE_FILTER: &str = "sandbox.lease.usage.*";

/// Client-facing request for a tenant's usage summed over its live leases (see
/// [`trogon_sandbox_contract::GetTenantUsageRequest`]).
pub const TENANT_USAGE_REQUEST: &str = "sandbox.tenant.usage";

/// Client-facing request for a tenant's limits and current consumption (see
/// [`trogon_sandbox_contract::GetAccountLimitsRequest`]).
pub const TENANT_LIMITS_REQUEST: &str = "sandbox.tenant.limits";

/// Client-facing request to run an arbitrary command inside an
/// already-running lease's sandbox (see [`trogon_sandbox_contract::RunCommandRequest`]).
pub fn lease_run_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.run_command.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_run_command`] requests.
pub const LEASE_RUN_COMMAND_FILTER: &str = "sandbox.lease.run_command.*";

/// Client-facing request to send a Unix signal to a previously started
/// command (see [`trogon_sandbox_contract::KillCommandRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_kill_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.kill_command.{}", lease.as_str())
}

pub fn lease_cancel_command_launch(lease: &LeaseId) -> String {
    format!("sandbox.lease.cancel_command_launch.{}", lease.as_str())
}

pub const LEASE_CANCEL_COMMAND_LAUNCH_FILTER: &str = "sandbox.lease.cancel_command_launch.*";

/// Wildcard subscription for [`lease_kill_command`] requests.
pub const LEASE_KILL_COMMAND_FILTER: &str = "sandbox.lease.kill_command.*";

/// Client-facing request to stop a previously started command gracefully
/// (see [`trogon_sandbox_contract::StopCommandRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_stop_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.stop_command.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_stop_command`] requests.
pub const LEASE_STOP_COMMAND_FILTER: &str = "sandbox.lease.stop_command.*";

/// Client-facing request to run a previously started command again (see
/// [`trogon_sandbox_contract::RestartCommandRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_restart_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.restart_command.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_restart_command`] requests.
pub const LEASE_RESTART_COMMAND_FILTER: &str = "sandbox.lease.restart_command.*";

/// Client-facing request to read a command's retained output in pages (see
/// [`trogon_sandbox_contract::ReadLogsRequest`]). The broker answers it from the command
/// log stream, so it is not forwarded to a worker.
pub fn lease_read_logs(lease: &LeaseId) -> String {
    format!("sandbox.lease.read_logs.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_read_logs`] requests.
pub const LEASE_READ_LOGS_FILTER: &str = "sandbox.lease.read_logs.*";

/// Client-facing request to write to the stdin of a previously started
/// interactive command (see [`trogon_sandbox_contract::WriteStdinRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_write_stdin(lease: &LeaseId) -> String {
    format!("sandbox.lease.write_stdin.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_write_stdin`] requests.
pub const LEASE_WRITE_STDIN_FILTER: &str = "sandbox.lease.write_stdin.*";

/// Client-facing request to open, feed, resize or close an interactive
/// terminal (see [`trogon_sandbox_contract::TerminalRequest`]). Routed like
/// [`lease_run_command`].
pub fn lease_terminal(lease: &LeaseId) -> String {
    format!("sandbox.lease.terminal.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_terminal`] requests.
pub const LEASE_TERMINAL_FILTER: &str = "sandbox.lease.terminal.*";

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

/// Wildcard subscription for [`worker_lease_activity`] signals.
pub const WORKER_LEASE_ACTIVITY_FILTER: &str = "sandbox.worker.activity.*";

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

/// Wildcard subscription for [`worker_authority_check`] requests.
pub const WORKER_AUTHORITY_FILTER: &str = "sandbox.worker.authority.*";

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

/// Wildcard subscription for [`lease_get_domain`] requests.
pub const LEASE_GET_DOMAIN_FILTER: &str = "sandbox.lease.get_domain.*";

/// Client-facing request for the vendor's own handle on a lease (see
/// [`trogon_sandbox_contract::NativeHandleRequest`]).
pub fn lease_native_handle(lease: &LeaseId) -> String {
    format!("sandbox.lease.native_handle.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_native_handle`] requests.
pub const LEASE_NATIVE_HANDLE_FILTER: &str = "sandbox.lease.native_handle.*";

/// Client-facing request for direct SSH access to a lease (see
/// [`trogon_sandbox_contract::SshAccessRequest`]).
pub fn lease_ssh_access(lease: &LeaseId) -> String {
    format!("sandbox.lease.ssh_access.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_ssh_access`] requests.
pub const LEASE_SSH_ACCESS_FILTER: &str = "sandbox.lease.ssh_access.*";

/// Client-facing request to poll the status of a previously started detached
/// command (see [`trogon_sandbox_contract::PollCommandRequest`]).
pub fn lease_poll_command(lease: &LeaseId) -> String {
    format!("sandbox.lease.poll_command.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_poll_command`] requests.
pub const LEASE_POLL_COMMAND_FILTER: &str = "sandbox.lease.poll_command.*";

/// Client-facing request to list the named or probed processes of a lease
/// (see [`trogon_sandbox_contract::ListProcessesRequest`]).
pub fn lease_list_processes(lease: &LeaseId) -> String {
    format!("sandbox.lease.list_processes.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_list_processes`] requests.
pub const LEASE_LIST_PROCESSES_FILTER: &str = "sandbox.lease.list_processes.*";

/// Client-facing request to wait for a process's readiness probe (see
/// [`trogon_sandbox_contract::WaitReadyRequest`]).
pub fn lease_wait_ready(lease: &LeaseId) -> String {
    format!("sandbox.lease.wait_ready.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_wait_ready`] requests.
pub const LEASE_WAIT_READY_FILTER: &str = "sandbox.lease.wait_ready.*";

/// Client-facing request to wait until a sandbox is usable (see
/// [`trogon_sandbox_contract::WaitSandboxReadyRequest`]).
pub fn lease_wait_sandbox_ready(lease: &LeaseId) -> String {
    format!("sandbox.lease.wait_sandbox_ready.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_wait_sandbox_ready`] requests.
pub const LEASE_WAIT_SANDBOX_READY_FILTER: &str = "sandbox.lease.wait_sandbox_ready.*";

/// Client-facing request to write one or more files into an already-running
/// lease's sandbox (see [`trogon_sandbox_contract::WriteFilesRequest`]).
pub fn lease_write_files(lease: &LeaseId) -> String {
    format!("sandbox.lease.write_files.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_write_files`] requests.
pub const LEASE_WRITE_FILES_FILTER: &str = "sandbox.lease.write_files.*";

/// Client-facing request to read a single file out of an already-running
/// lease's sandbox (see [`trogon_sandbox_contract::ReadFileRequest`]).
pub fn lease_read_file(lease: &LeaseId) -> String {
    format!("sandbox.lease.read_file.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_read_file`] requests.
pub const LEASE_READ_FILE_FILTER: &str = "sandbox.lease.read_file.*";

/// Client-facing request for one structured file operation inside an
/// already-running lease's sandbox (see [`trogon_sandbox_contract::FileOpRequest`]).
pub fn lease_file_op(lease: &LeaseId) -> String {
    format!("sandbox.lease.file_op.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_file_op`] requests.
pub const LEASE_FILE_OP_FILTER: &str = "sandbox.lease.file_op.*";

/// Client-facing request to create a per-sandbox Linux user inside an
/// already-running lease's sandbox (see [`trogon_sandbox_contract::CreateSandboxUserRequest`]).
pub fn lease_create_user(lease: &LeaseId) -> String {
    format!("sandbox.lease.create_user.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_create_user`] requests.
pub const LEASE_CREATE_USER_FILTER: &str = "sandbox.lease.create_user.*";

/// Client-facing request to create a per-sandbox Linux group inside an
/// already-running lease's sandbox (see [`trogon_sandbox_contract::CreateSandboxGroupRequest`]).
pub fn lease_create_group(lease: &LeaseId) -> String {
    format!("sandbox.lease.create_group.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_create_group`] requests.
pub const LEASE_CREATE_GROUP_FILTER: &str = "sandbox.lease.create_group.*";

/// Client-facing request to add or remove a sandbox user's group membership
/// inside an already-running lease's sandbox (see
/// [`trogon_sandbox_contract::ModifyGroupMembershipRequest`]).
pub fn lease_modify_group_membership(lease: &LeaseId) -> String {
    format!("sandbox.lease.modify_group_membership.{}", lease.as_str())
}

/// Wildcard subscription for [`lease_modify_group_membership`] requests.
pub const LEASE_MODIFY_GROUP_MEMBERSHIP_FILTER: &str = "sandbox.lease.modify_group_membership.*";

/// Request/reply asked by a lease's egress proxy about a destination its
/// policy neither allows nor denies (see [`trogon_sandbox_contract::DecideRequest`]).
/// Shaped `sandbox.egress.decide.{lease_id}` so one decider service can answer
/// every lease with [`EGRESS_DECIDE_FILTER`].
pub fn egress_decide(lease: &LeaseId) -> String {
    format!("sandbox.egress.decide.{}", lease.as_str())
}

/// Wildcard subscription for [`egress_decide`] requests.
pub const EGRESS_DECIDE_FILTER: &str = "sandbox.egress.decide.*";

/// Request/reply asked by a lease's egress proxy about one HTTP request to an
/// admitted destination (see [`trogon_sandbox_contract::RequestDecideRequest`]). Shaped
/// `sandbox.egress.request.{lease_id}`.
pub fn egress_request_decide(lease: &LeaseId) -> String {
    format!("sandbox.egress.request.{}", lease.as_str())
}

/// Wildcard subscription for [`egress_request_decide`] requests.
pub const EGRESS_REQUEST_DECIDE_FILTER: &str = "sandbox.egress.request.*";

/// Published by a lease's egress proxy once per denied connection, subject to
/// per-lease rate limiting (see [`trogon_sandbox_contract::EgressViolation`]). Shaped
/// `sandbox.egress.violation.{lease_id}` so alerting and audit subscribers can
/// take every lease with [`EGRESS_VIOLATION_FILTER`].
pub fn egress_violation(lease: &LeaseId) -> String {
    format!("sandbox.egress.violation.{}", lease.as_str())
}

/// Wildcard subscription for [`egress_violation`] events.
pub const EGRESS_VIOLATION_FILTER: &str = "sandbox.egress.violation.*";

/// Published by a lease's worker once per refused filesystem operation, subject
/// to per-lease rate limiting (see [`trogon_sandbox_contract::FsViolation`]). Shaped
/// `sandbox.fs.violation.{lease_id}`, alongside [`egress_violation`].
pub fn fs_violation(lease: &LeaseId) -> String {
    format!("sandbox.fs.violation.{}", lease.as_str())
}

/// Wildcard subscription for [`fs_violation`] events.
pub const FS_VIOLATION_FILTER: &str = "sandbox.fs.violation.*";

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

/// Subject prefix of the credential audit events. The JetStream stream that
/// stores them is named by [`credential_audit_stream_name`], because stream
/// names may not contain `.`.
pub const CREDENTIAL_AUDIT_STREAM: &str = "sandbox.audit.credentials";

/// JetStream stream name for the credential audit events (`.` is reserved for
/// subjects, so the subject prefix is spelled with `_`).
pub fn credential_audit_stream_name() -> String {
    CREDENTIAL_AUDIT_STREAM.replace('.', "_")
}

pub const SNAPSHOT_TAG_AUDIT_STREAM: &str = "sandbox.audit.snapshot-tag";

/// JetStream KV bucket for tenant snapshot tag records.
///
/// Logical path per tag: `sandbox.broker.kv.snapshot.tag.{tenant}.{name}`.
pub const SNAPSHOT_TAG_KV_BUCKET: &str = "sandbox-snapshot-tag";

/// JetStream KV bucket recording digest resolved from a named tag at lease dispatch.
///
/// Logical path: `sandbox.broker.kv.lease.{lease_id}.resolved_digest`.
pub const LEASE_RESOLVED_DIGEST_KV_BUCKET: &str = "sandbox-lease-resolved-digest";

/// JetStream KV bucket mapping a tenant-scoped lease `name` to the lease ID
/// currently holding it, so a later request with the same name can reconnect
/// to (or resume from a snapshot tagged for) the same logical sandbox.
///
/// Logical path: `sandbox.broker.kv.lease.name.{tenant}.{name}`.
pub const LEASE_NAME_INDEX_KV_BUCKET: &str = "sandbox-lease-name-index";

/// JetStream KV bucket recording the last time each snapshot digest was used
/// to resolve a lease, so retention age can reset on use rather than only on
/// tag reassignment.
pub const SNAPSHOT_LAST_USED_KV_BUCKET: &str = "sandbox-snapshot-last-used";

/// JetStream KV bucket recording which tenant took each provider-native
/// snapshot, so a restore is limited to the tenant's own.
pub const NATIVE_SNAPSHOT_KV_BUCKET: &str = "sandbox-native-snapshots";

/// JetStream KV bucket recording whether a lease's snapshot capture succeeded
/// or failed, so failed captures are queryable per-lease instead of only
/// visible via logs and aggregate metrics.
///
/// Logical path: `sandbox.broker.kv.lease.{lease_id}.capture_status`.
pub const SNAPSHOT_CAPTURE_STATUS_KV_BUCKET: &str = "sandbox-snapshot-capture-status";

pub fn snapshot_tag_audit_subject(tenant: &TenantId, name: &SnapshotTagName) -> String {
    format!("{SNAPSHOT_TAG_AUDIT_STREAM}.{}.{}", tenant.as_str(), name.as_str())
}

/// JetStream KV bucket for tenant drive records.
///
/// Logical path per drive: `sandbox.broker.kv.drive.{tenant}.{name}`.
pub const DRIVE_KV_BUCKET: &str = "sandbox-drive";

/// JetStream KV bucket for persistent sandbox entity records.
///
/// Key per entity: `e.{hex(tenant)}.{ulid}`.
pub const ENTITY_KV_BUCKET: &str = "sandbox-entities";

/// JetStream stream recording entity lifecycle events, capped per subject.
pub const ENTITY_EVENT_STREAM: &str = "sandbox-entity-events";

/// Subject prefix of entity events: `sandbox.entity.event.{hex(tenant)}.{ulid}`.
pub const ENTITY_EVENT_SUBJECT_PREFIX: &str = "sandbox.entity.event";

/// Client-facing request/reply to read one entity by id or name (see
/// [`trogon_sandbox_contract::GetEntityRequest`]). Not scoped by lease id: an entity
/// outlives its leases.
pub const ENTITY_GET_REQUEST: &str = "sandbox.entity.get";

/// Client-facing request/reply to list a tenant's entities (see
/// [`trogon_sandbox_contract::ListEntitiesRequest`]).
pub const ENTITY_LIST_REQUEST: &str = "sandbox.entity.list";

/// Client-facing request/reply to delete an entity (see
/// [`trogon_sandbox_contract::DeleteEntityRequest`]).
pub const ENTITY_DELETE_REQUEST: &str = "sandbox.entity.delete";

/// JetStream KV bucket mapping (tenant, name) to an entity id.
///
/// Key per name: `n.{hex(tenant)}.{hex(name)}`.
pub const ENTITY_NAME_INDEX_KV_BUCKET: &str = "sandbox-entity-names";

/// JetStream KV bucket for per-lease usage counters, keyed by lease id. Kept
/// beside the active-lease bucket so every broker replica reads the same
/// figures, and retained after release so a released lease reports its final
/// figures until the bucket ages them out.
pub const LEASE_USAGE_KV_BUCKET: &str = "sandbox-lease-usage";

/// JetStream KV bucket holding a marker for every delete-by-id whose purge has
/// not finished, keyed by lease id. The marker outlives the active-lease
/// record, so a retried delete and the broker's reconciler can still find the
/// name whose snapshot tag must go.
pub const LEASE_PENDING_PURGE_KV_BUCKET: &str = "sandbox-lease-pending-purge";

/// JetStream KV bucket holding a marker for every lease release that removed,
/// or is about to remove, the active-lease record and has not finished
/// refunding quota and announcing, keyed by lease id. Kept apart from the
/// pending-purge bucket so a release that is not a purge is never mistaken for
/// one, which would remove state a plain release keeps on purpose.
pub const LEASE_PENDING_RELEASE_KV_BUCKET: &str = "sandbox-lease-pending-release";

/// KV bucket names allow only letters, digits, `-`, `_` and `=`; a `.` is
/// rejected by the client, so the broker could not start with the dotted name.
pub fn credential_kv_bucket(provider: CredentialProvider) -> String {
    format!("sandbox_credentials_kv_{}", provider.as_str())
}

pub const CREDENTIAL_TOKEN_BUCKET: &str = "sandbox_credentials_kv_token";
