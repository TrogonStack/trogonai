#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod egress;
pub mod nats_todo_tool;
pub mod permission;
pub mod permission_rules;
pub mod session_store;
pub mod spawn_agent_tool;
pub mod wasm_bash_tool;

pub use permission::{ChannelPermissionChecker, PermissionReq, PermissionTx, RulesPermissionChecker};
pub use session_store::{
    AuditEntry, AuditOutcome, NatsSessionStore, SessionState, SessionStore, StoredMcpServer,
    TodoItem, append_audit_entries,
};
