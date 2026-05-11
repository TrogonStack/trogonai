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
pub use elicitation::{ElicitationReq, ElicitationTx};
pub use session_notifier::{NatsSessionNotifier, PromptEventClient, SessionNotifier};
pub use trogon_runner_tools::{
    ChannelPermissionChecker, NatsSessionStore, PermissionReq, PermissionTx, SessionState,
    SessionStore, StoredMcpServer,
};

#[cfg(feature = "test-helpers")]
pub use trogon_runner_tools::session_store;
