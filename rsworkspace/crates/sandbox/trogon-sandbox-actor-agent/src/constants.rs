//! Paths of the HTTP contract between a worker and the in-actor agent.
//!
//! The substrate has no exec, attach, or port-forward RPC, so everything the sandbox surface needs travels over the
//! actor's own HTTP ingress. Request and response bodies are the JSON encodings of the contract types, `lease_id`
//! included: the actor outlives any single dispatch, so every call carries the lease it believes it is talking to and
//! the agent refuses a mismatch rather than running a command meant for a lease that no longer owns the sandbox.

#[cfg(feature = "server")]
use std::time::Duration;

/// Reports HTTP server liveness only.
///
/// The runtime's wakeup probe gates resume and the golden-template warmup on this returning 200, and nothing ever
/// binds a lease during either, so this must not require a bind; lease identity is enforced per request instead.
pub const READYZ_PATH: &str = "/readyz";
pub const BIND_PATH: &str = "/_sandbox/v1/lease/bind";
pub const EXEC_PATH: &str = "/_sandbox/v1/exec";
pub const EXEC_POLL_PATH: &str = "/_sandbox/v1/exec/poll";
pub const EXEC_KILL_PATH: &str = "/_sandbox/v1/exec/kill";
pub const EXEC_WRITE_STDIN_PATH: &str = "/_sandbox/v1/exec/write_stdin";
pub const FILES_WRITE_PATH: &str = "/_sandbox/v1/files/write";
pub const FILES_READ_PATH: &str = "/_sandbox/v1/files/read";
/// Opens a terminal.
///
/// The reply is a newline-delimited [`crate::TerminalFrame`] stream that stays open for the terminal's life; closing
/// the connection hangs the terminal up.
pub const TERMINAL_OPEN_PATH: &str = "/_sandbox/v1/terminal/open";
/// Input, resize and close for an open terminal. One JSON request, one JSON reply.
pub const TERMINAL_OP_PATH: &str = "/_sandbox/v1/terminal/op";
pub const IDENTITY_PATH: &str = "/_sandbox/v1/identity";
pub const USERS_CREATE_PATH: &str = "/_sandbox/v1/users/create";
pub const GROUPS_CREATE_PATH: &str = "/_sandbox/v1/groups/create";
pub const GROUP_MEMBERSHIP_PATH: &str = "/_sandbox/v1/groups/membership";

/// Path prefix the agent owns.
///
/// Everything outside it is reverse-proxied to the tenant's exposed port, so a tenant workload can serve its own routes
/// on the same hostname.
pub const RESERVED_PATH_PREFIX: &str = "/_sandbox/";

/// Where the runtime writes the actor's identity.
///
/// It is read fresh on every use: the value is written per resume, so a copy cached before a suspend names the
/// previous actor.
pub const ACTOR_ID_PATH: &str = "/run/ate/actor-id";

/// Version of this HTTP contract.
///
/// It is bumped when a path, a body shape or a header changes in a way an older peer cannot ignore. Additive,
/// serde-defaulted fields do not bump it.
pub const GUEST_PROTOCOL_VERSION: u32 = 1;

/// Oldest worker the agent still serves.
///
/// A worker that predates the handshake sends no version header and counts as version 0, so it is refused by default.
/// An operator who still runs an older worker against a new guest image sets [`MIN_WORKER_PROTOCOL_VERSION_ENV`] to 0
/// on the agent until the workers are upgraded.
pub const MIN_SUPPORTED_WORKER_PROTOCOL_VERSION: u32 = 1;

/// Oldest agent the worker still drives, from the same rule applied the other way.
///
/// An operator who still boots old guest images sets [`MIN_AGENT_PROTOCOL_VERSION_ENV`] to 0 on the worker until the
/// images are rebuilt.
pub const MIN_SUPPORTED_AGENT_PROTOCOL_VERSION: u32 = 1;

/// Environment variable that overrides [`MIN_SUPPORTED_WORKER_PROTOCOL_VERSION`] on the agent.
///
/// Unset, empty or unparseable keeps the default.
pub const MIN_WORKER_PROTOCOL_VERSION_ENV: &str = "SANDBOX_MIN_WORKER_PROTOCOL_VERSION";

/// Environment variable that overrides [`MIN_SUPPORTED_AGENT_PROTOCOL_VERSION`] on the worker.
///
/// Unset, empty or unparseable keeps the default.
pub const MIN_AGENT_PROTOCOL_VERSION_ENV: &str = "SANDBOX_MIN_AGENT_PROTOCOL_VERSION";

/// Request header carrying the caller's [`GUEST_PROTOCOL_VERSION`].
pub const WORKER_PROTOCOL_VERSION_HEADER: &str = "x-sandbox-protocol-version";

/// Response header carrying the agent's [`GUEST_PROTOCOL_VERSION`].
///
/// It is present on every response to a path under the reserved prefix and on the readiness path.
pub const AGENT_PROTOCOL_VERSION_HEADER: &str = "x-sandbox-agent-protocol-version";

/// Response header carrying the oldest worker version the agent serves.
pub const AGENT_MIN_PROTOCOL_VERSION_HEADER: &str = "x-sandbox-agent-min-protocol-version";

/// HTTP status of a [`crate::GuestProtocolError`] refusal (`Upgrade Required`).
pub const PROTOCOL_REFUSED_STATUS: u16 = 426;

/// HTTP status an agent answers for a verb it knows it does not implement.
pub const UNIMPLEMENTED_STATUS: u16 = 501;

#[cfg(feature = "server")]
/// Where a queued write stands, shared between the caller waiting on it and the task that performs it.
pub(crate) const PHASE_QUEUED: u8 = 0;

#[cfg(feature = "server")]
pub(crate) const PHASE_STARTED: u8 = 1;

#[cfg(feature = "server")]
pub(crate) const PHASE_CANCELLED: u8 = 2;

#[cfg(feature = "server")]
/// Frames buffered between the task that owns a child and the HTTP response reading them.
pub(crate) const EXEC_FRAME_BUFFER: usize = 64;

#[cfg(feature = "server")]
/// How many signals may wait for the task that owns a child.
pub(crate) const KILL_QUEUE_DEPTH: usize = 8;

#[cfg(feature = "server")]
/// Writes queued toward the process before callers see backpressure.
pub(crate) const STDIN_QUEUE_DEPTH: usize = 16;

#[cfg(feature = "server")]
/// The shell a file operation borrows when it has to run as a sandbox user.
pub(crate) const FILES_SHELL: &str = "/bin/sh";

#[cfg(feature = "server")]
pub(crate) const DEFAULT_SHELL: &str = "/bin/sh";

#[cfg(feature = "server")]
pub(crate) const DEFAULT_TERM: &str = "xterm-256color";

#[cfg(feature = "server")]
/// Frames buffered between the pump and the HTTP body. Small on purpose: the terminal buffers.
pub(crate) const TERMINAL_FRAME_BUFFER: usize = 8;

#[cfg(feature = "server")]
pub(crate) const INPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(feature = "server")]
/// How long a hung-up process group gets to exit before it is killed.
pub(crate) const HANGUP_GRACE: Duration = Duration::from_secs(2);

#[cfg(feature = "server")]
/// How long the pump keeps draining after the command itself has exited, for output held by a background process that
/// still has the slave open.
pub(crate) const DRAIN_IDLE: Duration = Duration::from_millis(100);

#[cfg(feature = "server")]
/// Stands in for "no limit" while the command is still running.
pub(crate) const NO_IDLE_LIMIT: Duration = Duration::from_secs(60 * 60 * 24 * 365);

#[cfg(feature = "server")]
pub(crate) const OUTPUT_READ_CHUNK: usize = 8 * 1024;

#[cfg(feature = "server")]
/// The shell every provisioning command runs through, probed like any other tool rather than assumed.
pub(crate) const PROVISIONING_SHELL: &str = "/bin/bash";
