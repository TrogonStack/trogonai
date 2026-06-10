#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod catalog_picker;
pub mod compaction;
pub mod compactor_wire;
pub mod egress;
pub mod elicitation;
pub mod mcp;
pub mod nats_todo_tool;
pub mod permission;
pub mod permission_bridge;
pub mod permission_rules;
pub mod portable_session;
pub mod session_proto;
pub mod session_store;

pub use session_proto::{decode_compaction, encode_compaction};
pub mod spawn_agent_tool;
pub mod spawn_session;
pub mod trogon_md;
pub mod wasm_bash_tool;
pub mod worktree;

pub use catalog_picker::{
    DEFAULT_COMPACTOR_LABEL, compactor_model_config_option, compactor_model_current_value,
    compactor_model_select_options, parse_compactor_config, session_window_from_catalog,
};
pub use compaction::{
    COMPACT_SUBJECT, CompactError, CompactProviders, DEFAULT_COMPACT_THRESHOLD_PCT, DEFAULT_TOKEN_BUDGET,
    compaction_settings_from_env, estimate_tokens, maybe_compact, over_threshold,
};
pub use compactor_wire::{CompactWireResponse, decode_compact_response, encode_compact_request};
pub use egress::EgressPolicy;
pub use elicitation::{
    ElicitationReq, ElicitationTx, answer_from_response, elicit_via_channel, handle_elicitation_request_nats,
};
pub use mcp::{build_session_mcp, convert_mcp_servers};
pub use permission::{
    ChannelPermissionChecker, ModePermissionChecker, PermissionReq, PermissionTx, RulesPermissionChecker,
    build_mode_permission_checker, check_tool_permission,
};
pub use permission_bridge::handle_permission_request_nats;
pub use portable_session::{
    EXPORT_VERSION_V2, ParsedExport, PortableBlock, PortableExportV2, PortableMessage, PortableMessageV2,
    export_json_from_wire, message_to_v2, messages_need_v2, messages_to_export_v2, messages_to_v1, parse_export_json,
    text_to_v2, v1_to_messages, v2_message_to_text, v2_to_messages,
};
pub use session_store::{
    AllowedToolsSessionStore, AuditEntry, AuditOutcome, NatsSessionStore, SessionState, SessionStore, StoredMcpServer,
    TodoItem, append_audit_entries,
};
pub use trogon_md::{
    FsTrogonMdLoader, TrogonMdLayer, TrogonMdLoading, list_trogon_md_hierarchy, load_trogon_md, project_trogon_md_path,
};

/// Guidance appended to every interactive runner's system prompt so the agent
/// retrieves URLs with the `fetch_url` tool instead of treating a link as a local
/// file path. Without it, a model asked to "see <url>" or "check example.com"
/// reaches for file/search tools and comes back empty.
pub const URL_FETCH_GUIDANCE: &str = "When the user gives or refers to a URL or web link (for example \"see https://example.com\", \"check example.com\", or \"open this page\"), call the fetch_url tool to retrieve its contents. Never treat a URL as a local file path or search the filesystem for it.";
