//! Subjects, queue groups, intervals and reply messages the worker serves with.

use std::time::Duration;

/// Lease dispatches share one queue group, so exactly one worker takes each.
pub const LEASE: &str = "sandbox.provider.e2b.lease";
pub const LEASE_QUEUE_GROUP: &str = "sandbox-provider-e2b";
pub const CANCEL: &str = "sandbox.provider.e2b.cancel.>";
pub const CANCEL_OWNED: &str = "sandbox.provider.e2b.cancel_owned.>";
pub const CHECKPOINT_RUNNING: &str = "sandbox.provider.e2b.checkpoint_running.>";
pub const NETWORK_POLICY_UPDATE: &str = "sandbox.provider.e2b.network_policy_update.>";
pub const RUN_COMMAND: &str = "sandbox.provider.e2b.run_command.>";
pub const CANCEL_COMMAND_LAUNCH: &str = "sandbox.provider.e2b.cancel_command_launch.>";
pub const POLL_COMMAND: &str = "sandbox.provider.e2b.poll_command.>";
pub const LIST_PROCESSES: &str = "sandbox.provider.e2b.list_processes.>";
pub const KILL_COMMAND: &str = "sandbox.provider.e2b.kill_command.>";
pub const WRITE_STDIN: &str = "sandbox.provider.e2b.write_stdin.>";
pub const WRITE_FILES: &str = "sandbox.provider.e2b.write_files.>";
pub const READ_FILE: &str = "sandbox.provider.e2b.read_file.>";
pub const FILE_OP: &str = "sandbox.provider.e2b.file_op.>";
pub const CREATE_USER: &str = "sandbox.provider.e2b.create_user.>";
pub const CREATE_GROUP: &str = "sandbox.provider.e2b.create_group.>";
pub const MODIFY_GROUP_MEMBERSHIP: &str = "sandbox.provider.e2b.modify_group_membership.>";
pub const GET_DOMAIN: &str = "sandbox.provider.e2b.get_domain.>";
pub const PAUSE: &str = "sandbox.provider.e2b.pause.>";
pub const RESUME: &str = "sandbox.provider.e2b.resume.>";
pub const SUSPEND: &str = "sandbox.provider.e2b.suspend.>";
pub const WAKE: &str = "sandbox.provider.e2b.wake.>";
pub const NATIVE_HANDLE: &str = "sandbox.provider.e2b.native_handle.>";

/// Native snapshot deletions share one queue group, so exactly one worker takes each.
pub const NATIVE_SNAPSHOT_DELETE_QUEUE_GROUP: &str = "sandbox-e2b-native-snapshot-delete";

/// How often the worker retries native snapshot deletions that are still pending.
pub const SNAPSHOT_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);

pub const SNAPSHOT_EXPORT_UNSUPPORTED: &str =
    "E2B owns its snapshots; exporting one as an OCI artifact is not supported";
pub const CHECKPOINT_NEEDS_NATIVE: &str = "E2B snapshots are provider-native; ask for a native snapshot";
pub const NETWORK_POLICY_AT_CREATE: &str = "E2B applies network settings at sandbox create time only";
pub const INVALID_COMMAND_SCOPE: &str = "command request lacks exact verified assignment authority";
pub const INVALID_CLEANUP_SCOPE: &str = "targeted cleanup requires exact verified assignment authority";
pub const LEDGER_UNAVAILABLE: &str = "targeted dispatch ledger is unavailable";
pub const ASSIGNMENT_REQUIRED: &str = "targeted dispatch requires an assignment";
pub const ASSIGNED_ON_SHARED_INTAKE: &str = "assigned dispatch must use the exact worker intake subject";

pub const INVALID_RUN_COMMAND: &str = "invalid run command request";
pub const INVALID_CANCEL_COMMAND_LAUNCH: &str = "invalid command launch cancellation request";
pub const INVALID_NATIVE_SNAPSHOT_DELETE: &str = "invalid native snapshot delete request";
