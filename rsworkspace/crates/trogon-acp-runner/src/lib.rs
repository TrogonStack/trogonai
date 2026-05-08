#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod agent;
pub mod agent_runner;
pub mod egress;
pub mod permission_rules;
pub mod wasm_bash_tool;
pub mod elicitation;
pub mod permission;
pub mod permission_bridge;
pub mod prompt_converter;
pub mod session_notifier;
pub mod session_store;

pub use agent::{GatewayConfig, TrogonAgent};
pub use agent_runner::AgentRunner;
pub use egress::EgressPolicy;
pub use elicitation::{ElicitationReq, ElicitationTx};
pub use permission::{ChannelPermissionChecker, PermissionReq, PermissionTx};
pub use session_notifier::{NatsSessionNotifier, PromptEventClient, SessionNotifier};
pub use session_store::{
    AuditEntry, AuditOutcome, NatsSessionStore, SessionState, SessionStore, StoredMcpServer,
    append_audit_entries,
};
