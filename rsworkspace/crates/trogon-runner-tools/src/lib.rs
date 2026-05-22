#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod compaction;
pub mod egress;
pub mod nats_todo_tool;
pub mod permission;
pub mod permission_bridge;
pub mod permission_rules;
pub mod portable_session;
pub mod trogon_md;
pub mod session_store;
pub mod spawn_agent_tool;
pub mod wasm_bash_tool;

pub use compaction::{
    compaction_settings_from_env, estimate_tokens, maybe_compact, over_threshold, CompactError,
    COMPACT_SUBJECT, DEFAULT_COMPACT_THRESHOLD_PCT, DEFAULT_TOKEN_BUDGET,
};
pub use egress::EgressPolicy;
pub use permission::{
    build_mode_permission_checker, check_tool_permission, ChannelPermissionChecker,
    ModePermissionChecker, PermissionReq, PermissionTx, RulesPermissionChecker,
};
pub use permission_bridge::handle_permission_request_nats;
pub use session_store::{
    AllowedToolsSessionStore, AuditEntry, AuditOutcome, NatsSessionStore, SessionState,
    SessionStore, StoredMcpServer, TodoItem, append_audit_entries,
};
pub use trogon_md::{FsTrogonMdLoader, TrogonMdLoading};
