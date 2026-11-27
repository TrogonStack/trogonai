//! The agent that runs inside every sandbox actor.
//!
//! The runtime exposes no exec RPC, so the sandbox exec surface reaches an actor through this process: the worker
//! calls it over the actor's HTTP ingress, and it answers with the same `trogon_sandbox_contract` types every other
//! provider replies with.
//!
//! The guest protocol types are always available. The HTTP server itself sits behind the `server` feature so a worker
//! that only speaks the protocol does not link it.

// `LeaseError` and `TerminalError` are the contract's own error types; their size is not this crate's to change.
#![allow(clippy::result_large_err)]

mod actor_id;
mod actor_id_error;
mod actor_identity;
mod bind_lease_request;
mod bind_lease_response;
mod constants;
mod exec_begin;
mod exec_frame;
mod guest_call_failure;
mod guest_entropy_reseed_method;
mod guest_entropy_reseed_receipt;
mod guest_entropy_seed;
mod guest_protocol_error;
mod guest_protocol_version;
mod terminal_frame;

#[cfg(feature = "server")]
mod actor_identity_error;
#[cfg(feature = "server")]
mod actor_identity_source;
#[cfg(feature = "server")]
mod agent_state;
#[cfg(feature = "server")]
mod binding_error;
#[cfg(feature = "server")]
mod binding_state;
#[cfg(feature = "server")]
mod command_builder;
#[cfg(feature = "server")]
mod entropy_reseed_error;
#[cfg(feature = "server")]
mod entropy_reseeder;
#[cfg(feature = "server")]
mod exec_registry;
#[cfg(feature = "server")]
mod execution;
#[cfg(feature = "server")]
mod files;
#[cfg(feature = "server")]
mod handlers;
#[cfg(feature = "server")]
mod kernel_entropy_reseeder;
#[cfg(feature = "server")]
mod launch;
#[cfg(feature = "server")]
mod lease_binding;
#[cfg(feature = "server")]
mod lease_fault;
#[cfg(feature = "server")]
mod ndjson;
#[cfg(feature = "server")]
mod output_collector;
#[cfg(feature = "server")]
mod protocol_admission;
#[cfg(feature = "server")]
mod reverse_proxy;
#[cfg(feature = "server")]
mod router;
#[cfg(feature = "server")]
mod script_outcome;
#[cfg(feature = "server")]
mod script_runner;
#[cfg(feature = "server")]
mod seed_receipt;
#[cfg(feature = "server")]
mod serve;
#[cfg(feature = "server")]
mod shell_quote;
#[cfg(feature = "server")]
mod spawn_failure;
#[cfg(feature = "server")]
mod stdin_apply_error;
#[cfg(feature = "server")]
mod stdin_deadlines;
#[cfg(feature = "server")]
mod stdin_handle;
#[cfg(feature = "server")]
mod stdin_op;
#[cfg(feature = "server")]
mod stdin_writer;
#[cfg(feature = "server")]
mod supervise;
#[cfg(feature = "server")]
mod syscall_result;
#[cfg(feature = "server")]
mod target_user;
#[cfg(feature = "server")]
mod terminal_pty;
#[cfg(feature = "server")]
mod terminal_session;
#[cfg(feature = "server")]
mod terminal_sessions;
#[cfg(feature = "server")]
mod unix_signals;
#[cfg(feature = "server")]
mod user_provisioning;

pub use actor_id::ActorId;
pub use actor_id_error::ActorIdError;
pub use actor_identity::ActorIdentity;
pub use bind_lease_request::BindLeaseRequest;
pub use bind_lease_response::BindLeaseResponse;
pub use constants::{
    ACTOR_ID_PATH, AGENT_MIN_PROTOCOL_VERSION_HEADER, AGENT_PROTOCOL_VERSION_HEADER, BIND_PATH, EXEC_KILL_PATH,
    EXEC_PATH, EXEC_POLL_PATH, EXEC_WRITE_STDIN_PATH, FILES_READ_PATH, FILES_WRITE_PATH, GROUP_MEMBERSHIP_PATH,
    GROUPS_CREATE_PATH, GUEST_PROTOCOL_VERSION, IDENTITY_PATH, MIN_AGENT_PROTOCOL_VERSION_ENV,
    MIN_SUPPORTED_AGENT_PROTOCOL_VERSION, MIN_SUPPORTED_WORKER_PROTOCOL_VERSION, MIN_WORKER_PROTOCOL_VERSION_ENV,
    PROTOCOL_REFUSED_STATUS, READYZ_PATH, RESERVED_PATH_PREFIX, TERMINAL_OP_PATH, TERMINAL_OPEN_PATH,
    UNIMPLEMENTED_STATUS, USERS_CREATE_PATH, WORKER_PROTOCOL_VERSION_HEADER,
};
pub use exec_begin::ExecBegin;
pub use exec_frame::ExecFrame;
pub use guest_call_failure::GuestCallFailure;
pub use guest_entropy_reseed_method::GuestEntropyReseedMethod;
pub use guest_entropy_reseed_receipt::GuestEntropyReseedReceipt;
pub use guest_entropy_seed::GuestEntropySeed;
pub use guest_protocol_error::GuestProtocolError;
pub use guest_protocol_version::{
    admit_agent_version, admit_protocol_version, min_protocol_version_from_env, parse_min_protocol_version,
    parse_protocol_version,
};
pub use terminal_frame::TerminalFrame;

#[cfg(feature = "server")]
pub use actor_identity_error::ActorIdentityError;
#[cfg(feature = "server")]
pub use actor_identity_source::ActorIdentitySource;
#[cfg(feature = "server")]
pub use agent_state::AgentState;
#[cfg(feature = "server")]
pub use binding_error::BindingError;
#[cfg(feature = "server")]
pub use binding_state::BindingState;
#[cfg(feature = "server")]
pub use entropy_reseed_error::EntropyReseedError;
#[cfg(feature = "server")]
pub use entropy_reseeder::EntropyReseeder;
#[cfg(feature = "server")]
pub use exec_registry::ExecRegistry;
#[cfg(feature = "server")]
pub use files::{read_file, write_files};
#[cfg(feature = "server")]
pub use kernel_entropy_reseeder::KernelEntropyReseeder;
#[cfg(feature = "server")]
pub use lease_binding::LeaseBinding;
#[cfg(feature = "server")]
pub use reverse_proxy::ReverseProxy;
#[cfg(feature = "server")]
pub use router::router;
#[cfg(feature = "server")]
pub use serve::{serve, serve_vsock};
#[cfg(feature = "server")]
pub use target_user::TargetUser;
#[cfg(feature = "server")]
pub use terminal_sessions::TerminalSessions;
#[cfg(feature = "server")]
pub use user_provisioning::{create_group, create_user, modify_group_membership};
