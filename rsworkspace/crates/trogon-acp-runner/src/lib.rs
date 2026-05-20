#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod agent;
pub mod agent_runner;
pub mod trogon_md;
pub mod elicitation;
pub mod permission_bridge;
pub mod prompt_converter;
pub mod session_notifier;

pub use agent::{GatewayConfig, TrogonAgent};
pub use agent_runner::AgentRunner;
pub use trogon_runner_tools::egress::EgressPolicy;
pub use elicitation::{ElicitationReq, ElicitationTx};
pub use session_notifier::{NatsSessionNotifier, PromptEventClient, SessionNotifier};
pub use trogon_runner_tools::{
    ChannelPermissionChecker, NatsSessionStore, PermissionReq, PermissionTx, SessionState,
    SessionStore, StoredMcpServer,
};
pub use trogon_runner_tools::session_store::{
    AuditEntry, AuditOutcome, append_audit_entries,
};

#[cfg(feature = "test-helpers")]
pub use trogon_runner_tools::session_store;
#[cfg(feature = "test-helpers")]
pub use trogon_runner_tools::egress;
#[cfg(feature = "test-helpers")]
pub use trogon_runner_tools::wasm_bash_tool;
#[cfg(feature = "test-helpers")]
pub use trogon_runner_tools::nats_todo_tool;
#[cfg(feature = "test-helpers")]
pub use trogon_runner_tools::spawn_agent_tool;
#[cfg(feature = "test-helpers")]
pub use trogon_runner_tools::permission;
