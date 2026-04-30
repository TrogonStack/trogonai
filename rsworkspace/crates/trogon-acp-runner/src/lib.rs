#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod agent;
pub mod agent_runner;
pub mod permission;
pub mod permission_bridge;
pub mod prompt_converter;
pub mod session_notifier;
pub mod session_store;

pub use agent::{GatewayConfig, TrogonAgent};
pub use agent_runner::AgentRunner;
pub use permission::{ChannelPermissionChecker, PermissionReq, PermissionTx};
pub use session_notifier::{NatsSessionNotifier, PromptEventClient, SessionNotifier};
pub use session_store::{NatsSessionStore, SessionState, SessionStore, StoredMcpServer};
