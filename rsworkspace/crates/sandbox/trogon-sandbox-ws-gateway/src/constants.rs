use std::time::Duration;

use trogon_sandbox_contract::ProviderKind;

/// KV bucket that holds minted session records.
pub const SESSION_BUCKET: &str = "sandbox-ws-kv-session";

/// Request header that carries the API key when a session is minted.
pub const API_KEY_HEADER: &str = "X-Sandbox-API-Key";

/// Route prefix of the preview proxy.
pub const PREVIEW_ROUTE_PREFIX: &str = "/v1/preview";

/// Websocket subprotocol prefix that carries the session token.
pub(crate) const WS_TOKEN_PROTOCOL_PREFIX: &str = "bearer.";

/// Longest request id a websocket client may use, in bytes.
pub(crate) const MAX_REQUEST_ID_LEN: usize = 128;

/// Requests one socket may have in flight at once.
pub(crate) const MAX_PENDING_REQUESTS: usize = 32;

/// Slots of [`MAX_PENDING_REQUESTS`] only control tools may use, so a flood of work never blocks a cancel.
pub(crate) const RESERVED_CONTROL_REQUESTS: usize = 8;

/// Capacity of the per-socket outbound frame queue.
pub(crate) const OUTBOUND_QUEUE_CAPACITY: usize = 256;

/// Terminals one socket may hold open at once.
pub(crate) const MAX_TERMINALS_PER_SOCKET: usize = 32;

/// How long closing a terminal on socket drop may take.
pub(crate) const TERMINAL_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a worker heartbeat keeps a provider counted as live.
pub(crate) const HEARTBEAT_FRESHNESS: Duration = Duration::from_secs(30);

/// Random bytes behind a session token.
pub(crate) const SESSION_TOKEN_BYTES: usize = 32;

/// Random bytes behind a terminal id.
pub(crate) const TERMINAL_ID_BYTES: usize = 16;

/// Key prefix the preview grants live under in the session store.
pub(crate) const PREVIEW_GRANT_PREFIX: &str = "preview/";

/// Stream id of lease dashboard events.
pub const STREAM_LEASES: &str = "sandbox.leases";

/// Stream id of snapshot dashboard events.
pub const STREAM_SNAPSHOTS: &str = "sandbox.snapshots";

/// Stream id of worker dashboard events.
pub const STREAM_WORKERS: &str = "sandbox.workers";

/// Default KV bucket the broker records active leases in.
pub const DEFAULT_ACTIVE_LEASE_BUCKET: &str = "sandbox-active-leases";

/// Subject filter that matches every worker heartbeat.
pub(crate) const WORKER_HEARTBEAT_ALL: &str = "sandbox.worker.heartbeat.>";

/// How long `sandbox_lease` waits for a grant when the payload names no timeout.
pub(crate) const DEFAULT_LEASE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `sandbox_wait_released` waits when the payload names no timeout.
pub(crate) const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(300);

/// Tools that cancel or stop work. They keep a reserved share of the pending request slots.
pub(crate) const CONTROL_TOOLS: &[&str] = &[
    "sandbox_cancel",
    "sandbox_kill_command",
    "sandbox_stop_command",
    "sandbox_cancel_command",
    "sandbox_cancel_command_launch",
    "sandbox_cancel_operation",
];

/// Tools that run beside other requests on the same socket instead of one at a time.
pub(crate) const CONCURRENT_TOOLS: &[&str] = &[
    "sandbox_lease",
    "sandbox_fork",
    "sandbox_run_command",
    "sandbox_poll_command",
    "sandbox_stop_lease",
    "sandbox_pause_lease",
    "sandbox_resume_lease",
    "sandbox_suspend_lease",
    "sandbox_wake_lease",
    "sandbox_delete_lease",
    "sandbox_snapshot_lease",
    "sandbox_snapshot_delete",
    "sandbox_native_snapshot_delete",
    "sandbox_wait_released",
    "sandbox_wait_ready",
    "sandbox_wait_sandbox_ready",
    "sandbox_read_logs",
    "sandbox_read_file",
    "sandbox_write_files",
    "sandbox_file_op",
    "sandbox_restart_command",
    "sandbox_get_operation",
    "sandbox_operation_capabilities",
];

/// Version tag of a preview token.
pub(crate) const PREVIEW_TOKEN_VERSION: &str = "v1";

/// Domain separation prefix mixed into every preview token signature.
pub(crate) const PREVIEW_MAC_CONTEXT: &[u8] = b"sandbox-preview-v1\n";

/// Shortest preview signing key accepted, in bytes.
pub(crate) const MIN_PREVIEW_KEY_BYTES: usize = 32;

/// Longest preview signing key id accepted, in characters.
pub(crate) const MAX_PREVIEW_KEY_ID_LEN: usize = 32;

/// How long the preview proxy waits to connect to an upstream.
pub(crate) const PREVIEW_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Shortest lifetime a preview token is given.
pub(crate) const MIN_PREVIEW_TTL: Duration = Duration::from_secs(1);

/// How long the port probe waits to connect.
pub(crate) const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long one port probe may take in total.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Pause before the second port probe. It doubles up to [`PROBE_MAX_PAUSE`].
pub(crate) const PROBE_FIRST_PAUSE: Duration = Duration::from_millis(200);

/// Longest pause between port probes.
pub(crate) const PROBE_MAX_PAUSE: Duration = Duration::from_secs(1);

/// Headers that describe one connection and are never forwarded by the preview proxy.
pub(crate) const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Environment variable that lists the preview signing keys as `kid=hexkey[,kid=hexkey...]`.
pub(crate) const PREVIEW_SIGNING_KEYS_ENV: &str = "SANDBOX_PREVIEW_SIGNING_KEYS";

/// Environment variable that picks which preview signing key signs new tokens.
pub(crate) const PREVIEW_ACTIVE_KEY_ID_ENV: &str = "SANDBOX_PREVIEW_ACTIVE_KEY_ID";

/// Every provider the gateway reports capabilities for.
pub(crate) const PROVIDER_KINDS: &[ProviderKind] = &[
    ProviderKind::Subprocess,
    ProviderKind::LocalDocker,
    ProviderKind::Daytona,
    ProviderKind::Modal,
    ProviderKind::K8sMicroVm,
    ProviderKind::Hyperlight,
    ProviderKind::V8Wasm,
    ProviderKind::KubeVirt,
    ProviderKind::AzureContainerApps,
    ProviderKind::Substrate,
    ProviderKind::Firecracker,
    ProviderKind::Fly,
    ProviderKind::Proxmox,
    ProviderKind::E2b,
    ProviderKind::Vercel,
    ProviderKind::Space,
    ProviderKind::Mock,
];
