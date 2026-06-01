#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod compaction;
pub mod egress;
pub mod elicitation;
pub mod mcp;
pub mod nats_todo_tool;
pub mod permission;
pub mod permission_bridge;
pub mod permission_rules;
pub mod portable_session;
pub mod trogon_md;
pub mod session_store;
pub mod spawn_agent_tool;
pub mod spawn_session;
pub mod wasm_bash_tool;
pub mod worktree;

pub use compaction::{
    compaction_settings_from_env, estimate_tokens, maybe_compact, over_threshold, CompactError,
    COMPACT_SUBJECT, DEFAULT_COMPACT_THRESHOLD_PCT, DEFAULT_TOKEN_BUDGET,
};
pub use egress::EgressPolicy;
pub use elicitation::{
    answer_from_response, elicit_via_channel, handle_elicitation_request_nats, ElicitationReq,
    ElicitationTx,
};
pub use mcp::{build_session_mcp, convert_mcp_servers};
pub use permission::{
    build_mode_permission_checker, check_tool_permission, ChannelPermissionChecker,
    ModePermissionChecker, PermissionReq, PermissionTx, RulesPermissionChecker,
};
pub use permission_bridge::handle_permission_request_nats;
pub use portable_session::{
    export_json_from_wire, message_to_v2, messages_need_v2, messages_to_export_v2,
    messages_to_v1, parse_export_json, text_to_v2, v1_to_messages, v2_message_to_text,
    v2_to_messages, ParsedExport, PortableBlock, PortableExportV2, PortableMessage,
    PortableMessageV2, EXPORT_VERSION_V2,
};
pub use session_store::{
    AllowedToolsSessionStore, AuditEntry, AuditOutcome, NatsSessionStore, SessionState,
    SessionStore, StoredMcpServer, TodoItem, append_audit_entries,
};
pub use trogon_md::{FsTrogonMdLoader, TrogonMdLayer, TrogonMdLoading, list_trogon_md_hierarchy, load_trogon_md, project_trogon_md_path};

/// Guidance appended to every interactive runner's system prompt so the agent
/// retrieves URLs with the `fetch_url` tool instead of treating a link as a local
/// file path. Without it, a model asked to "see <url>" or "check example.com"
/// reaches for file/search tools and comes back empty.
pub const URL_FETCH_GUIDANCE: &str = "When the user gives or refers to a URL or web link (for example \"see https://example.com\", \"check example.com\", or \"open this page\"), call the fetch_url tool to retrieve its contents. Never treat a URL as a local file path or search the filesystem for it.";

/// Guidance appended to the system prompt while the session is in `plan` mode.
/// The permission layer already denies write tools and write-bash in plan mode,
/// but denial alone makes the model flail (it attempts an edit, gets rejected,
/// and reacts). This steers the model to behave like a planner: research
/// read-only, then present a plan and stop until the user approves leaving plan
/// mode. Mirrors Claude Code's plan mode behaviour.
pub const PLAN_MODE_GUIDANCE: &str = "You are currently in PLAN MODE. Do NOT make any changes yet: do not edit, create, or delete files, and do not run commands that modify state (these are blocked and will be rejected). First investigate the codebase using read-only tools (read_file, list_dir, glob, search_files, read-only git/bash) until you fully understand the task. Then, when you have a complete plan, call the ExitPlanMode tool with your plan to ask the user to approve leaving plan mode. Do not begin implementing until that approval is granted.";
