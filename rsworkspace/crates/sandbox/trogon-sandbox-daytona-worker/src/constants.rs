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
    "class",
    "gpu",
];

/// The largest interval, in minutes, a native option may carry: one year.
pub const MAX_INTERVAL_MINUTES: u64 = 525_600;

/// The sandbox classes Daytona offers.
pub const SANDBOX_CLASSES: &[&str] = &["small", "medium", "large"];

/// The most GPUs a sandbox may ask for.
pub const MAX_GPUS: u64 = 8;

/// The label prefix a lease tag travels under on the sandbox.
pub const TAG_LABEL_PREFIX: &str = "trogon.tag.";

/// The label that names the lease a sandbox belongs to.
pub const LEASE_LABEL: &str = "trogon.lease_id";

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
pub const SSH_ACCESS: &str = "sandbox.provider.daytona.ssh_access.>";

/// The user name a token travels under when a git host takes the token as a password.
pub const CLONE_TOKEN_USER: &str = "oauth2";

/// The mode a created workspace directory gets.
pub const WORKSPACE_MODE: u32 = 0o755;

/// How long SSH access lasts when the caller does not say, matching Daytona's own default.
pub const DEFAULT_SSH_ACCESS_MINUTES: u32 = 60;

/// Most leases this worker keeps a gateway session for. Sessions exist only for leases the worker holds, so this is a
/// backstop, not a working limit.
pub const MAX_GUEST_SESSIONS: usize = 256;

/// How long a connection to Daytona's SSH gateway may take, login included.
pub const GATEWAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a released session waits for the gateway to acknowledge the disconnect.
pub const GATEWAY_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// A gateway token is replaced this long before it expires.
pub const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// How long a token with no reported expiry is trusted. Daytona's default access lifetime is an hour.
pub const UNDATED_TOKEN_LIFETIME: Duration = Duration::from_secs(50 * 60);

/// How often a native snapshot is looked at while it builds, unless the operator says otherwise.
pub const DEFAULT_SNAPSHOT_POLL_INTERVAL: Duration = Duration::from_secs(DEFAULT_SNAPSHOT_POLL_INTERVAL_SECS);

/// How long a native snapshot may take to turn active, unless the operator says otherwise.
pub const DEFAULT_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(DEFAULT_SNAPSHOT_TIMEOUT_SECS);

/// The default for `--snapshot-poll-interval-secs`.
pub const DEFAULT_SNAPSHOT_POLL_INTERVAL_SECS: u64 = 2;

/// The default for `--snapshot-timeout-secs`.
pub const DEFAULT_SNAPSHOT_TIMEOUT_SECS: u64 = 300;

/// How long a guest script may run, in seconds.
pub(crate) const SCRIPT_TIMEOUT_SECONDS: u64 = 20;

/// The most output a guest script may print.
pub(crate) const MAX_SCRIPT_OUTPUT_BYTES: usize = 4096;

/// Most finished results kept at once; the oldest go first, so a lease that runs many short commands cannot grow the
/// worker's memory until it ends.
pub(crate) const MAX_COMPLETED_RESULTS: usize = 256;

/// Why a restricted egress policy is refused at admission.
pub(crate) const RESTRICTED_EGRESS_UNVERIFIED: &str =
    "restricted Daytona egress requires verified provider enforcement, which is unavailable";

pub(crate) const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_BRIDGE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const BRIDGE: &str = include_str!("ordinary_command_runtime/bridge.cjs");

/// The largest stdin payload sent in one frame, before encoding.
pub(crate) const STDIN_PIECE_BYTES: usize = 192 * 1024;

/// How often the acknowledgement file is read while a frame is outstanding.
pub(crate) const ACK_POLL: Duration = Duration::from_millis(50);

/// The line that tells the stdin reader of an interactive command to close the command's stdin.
pub(crate) const STDIN_EOF_LINE: &str = ".";

/// The port `ssh` uses when the command line names none.
pub(crate) const DEFAULT_PORT: u16 = 22;

/// Broadcast subject the broker sends terminal requests on for this provider
/// (see [`trogon_sandbox_protocol::subjects::provider_terminal`]).
pub const TERMINAL_SUBJECT: &str = "sandbox.provider.daytona.terminal.>";
