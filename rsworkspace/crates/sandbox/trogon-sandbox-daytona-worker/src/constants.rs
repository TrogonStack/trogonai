//! Values shared across the worker.

use std::time::Duration;

/// Where the Daytona REST API lives unless the operator points the worker elsewhere.
pub const DEFAULT_API_URL: &str = "https://app.daytona.io/api";

/// Native options this worker accepts under `native.daytona`.
pub const NATIVE_OPTION_KEYS: &[&str] = &[
    "auto_stop_interval",
    "auto_archive_interval",
    "auto_delete_interval",
    "public",
];

/// The largest interval, in minutes, a native option may carry: one year.
pub const MAX_INTERVAL_MINUTES: u64 = 525_600;

pub const UNSUPPORTED: &str = "Daytona owns its snapshots; exporting one as an OCI artifact is not supported";

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub const MAX_ID_LEN: usize = 128;
pub const MAX_SNAPSHOT_NAME_LEN: usize = 255;

pub const MESSAGE_LIMIT: usize = 300;

/// The number of sandboxes whose toolbox proxy URL the client remembers.
pub const PROXY_CACHE_LIMIT: usize = 1024;

/// Where a leased sandbox keeps a cloned workspace, and where a command without a cwd runs.
pub const WORKSPACE_PATH: &str = "/workspace";

/// Daytona accepts at most this many CIDR entries in a network allow list.
pub const MAX_ALLOW_ENTRIES: usize = 10;

/// How much longer than the command's own timeout a toolbox call may take, so the sandbox-side timeout fires first.
pub const TOOLBOX_DEADLINE_SLACK: Duration = Duration::from_secs(10);

/// The header that carries a port preview token.
pub const PREVIEW_TOKEN_HEADER: &str = "x-daytona-preview-token";

/// The exit status of a poll script when the command it asks about has not finished.
pub const RUNNING_EXIT: i32 = 3;

/// How long a sandbox may take to reach the state a lease asked for.
pub const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the worker waits between two looks at a sandbox that is changing state.
pub const STATE_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The user a toolbox command runs as when the lease names none, and the user whose home is the default directory.
pub const ROOT_USER: &str = "root";

/// The longest a revision or commit name may be, so it can never be mistaken for an option.
pub const MAX_SHA_LEN: usize = 64;

/// How long a finished command stays pollable after its result is read.
pub const COMPLETED_RESULT_TTL: Duration = Duration::from_secs(600);

/// Where the sandbox keeps the files of one detached command.
pub const COMMAND_FILE_PREFIX: &str = "/tmp/.trogon-exec-";

/// Lease dispatches share one queue group, so exactly one worker takes each.
pub const LEASE: &str = "sandbox.provider.daytona.lease";
pub const LEASE_QUEUE_GROUP: &str = "sandbox-provider-daytona";
pub const CANCEL: &str = "sandbox.provider.daytona.cancel.>";
pub const CHECKPOINT_RUNNING: &str = "sandbox.provider.daytona.checkpoint_running.>";
pub const NETWORK_POLICY_UPDATE: &str = "sandbox.provider.daytona.network_policy_update.>";
pub const RUN_COMMAND: &str = "sandbox.provider.daytona.run_command.>";
pub const POLL_COMMAND: &str = "sandbox.provider.daytona.poll_command.>";
pub const KILL_COMMAND: &str = "sandbox.provider.daytona.kill_command.>";
pub const WRITE_STDIN: &str = "sandbox.provider.daytona.write_stdin.>";
pub const WRITE_FILES: &str = "sandbox.provider.daytona.write_files.>";
pub const READ_FILE: &str = "sandbox.provider.daytona.read_file.>";
pub const FILE_OP: &str = "sandbox.provider.daytona.file_op.>";
pub const CREATE_USER: &str = "sandbox.provider.daytona.create_user.>";
pub const CREATE_GROUP: &str = "sandbox.provider.daytona.create_group.>";
pub const MODIFY_GROUP_MEMBERSHIP: &str = "sandbox.provider.daytona.modify_group_membership.>";
pub const GET_DOMAIN: &str = "sandbox.provider.daytona.get_domain.>";
pub const PAUSE: &str = "sandbox.provider.daytona.pause.>";
pub const RESUME: &str = "sandbox.provider.daytona.resume.>";
pub const SUSPEND: &str = "sandbox.provider.daytona.suspend.>";
pub const WAKE: &str = "sandbox.provider.daytona.wake.>";
pub const NATIVE_HANDLE: &str = "sandbox.provider.daytona.native_handle.>";

/// The user name a token travels under when a git host takes the token as a password.
pub const CLONE_TOKEN_USER: &str = "oauth2";

/// The mode a created workspace directory gets.
pub const WORKSPACE_MODE: u32 = 0o755;

/// Encoding room reserved for both streams' truncation markers: a marker is under 64 bytes, and the number-array
/// encoding costs up to four bytes per byte.
pub const MARKER_ROOM: usize = 2 * 64 * 4;
