//! Paths of the HTTP contract between a worker and the in-actor agent.
//!
//! The substrate has no exec, attach, or port-forward RPC, so everything the sandbox surface needs travels over the
//! actor's own HTTP ingress. Request and response bodies are the JSON encodings of the contract types, `lease_id`
//! included: the actor outlives any single dispatch, so every call carries the lease it believes it is talking to and
//! the agent refuses a mismatch rather than running a command meant for a lease that no longer owns the sandbox.

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
