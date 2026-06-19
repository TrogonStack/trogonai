#![cfg_attr(coverage, feature(coverage_attribute))]

pub mod compaction;
pub mod egress;
pub mod elicitation;
pub mod hooks;
pub mod mcp;
pub mod nats_todo_tool;
pub mod permission;
pub mod permission_bridge;
pub mod permission_rules;
pub mod portable_session;
pub mod safety_classifier;
pub mod scope;
pub mod session_store;
pub mod spawn_agent_tool;
pub mod spawn_session;
pub mod subagents;
pub mod trogon_md;
pub mod wasm_bash_tool;
pub mod worktree;

pub use compaction::{
    COMPACT_SUBJECT, CompactError, DEFAULT_COMPACT_THRESHOLD_PCT, DEFAULT_TOKEN_BUDGET, compaction_requested,
    compaction_settings_from_env, estimate_tokens, maybe_compact, over_threshold,
};
pub use egress::EgressPolicy;
pub use elicitation::{
    ElicitationReq, ElicitationTx, answer_from_response, elicit_via_channel, handle_elicitation_request_nats,
};
pub use hooks::{HookMatcher, HookOutcome, HookPostToolObserver, HooksConfig, run_event_hooks};
pub use mcp::{McpDispatch, SessionMcpCache, build_session_mcp, convert_mcp_servers};
pub use permission::{
    ChannelPermissionChecker, ClassifierVerdict, ModePermissionChecker, PermissionExtras, PermissionReq, PermissionTx,
    RulesPermissionChecker, SafetyClassifier, build_mode_permission_checker, check_tool_permission,
};
pub use permission_bridge::handle_permission_request_nats;
pub use portable_session::{
    EXPORT_VERSION_V2, ParsedExport, PortableBlock, PortableExportV2, PortableMessage, PortableMessageV2,
    export_json_from_wire, message_to_v2, messages_need_v2, messages_to_export_v2, messages_to_v1, parse_export_json,
    text_to_v2, v1_to_messages, v2_message_to_text, v2_to_messages,
};
pub use safety_classifier::{LlmSafetyClassifier, build_auto_safety_classifier};
pub use scope::{CommandSet, GlobSet, NetworkPolicy, OnExceed, Scope, ScopeDecision, ScopeError, ScopeWire};
pub use session_store::{
    AllowedToolsSessionStore, AuditEntry, AuditOutcome, BashJob, NatsSessionStore, SessionState, SessionStore,
    StoredMcpServer, TodoItem, append_audit_entries, filter_tool_defs_by_allowlist, intersect_enabled_tools,
    is_tool_in_allowlist, turn_tool_allowlist_from_prompt_meta,
};
pub use subagents::{SubagentDef, load_subagent, load_subagents, parse_subagent};
pub use trogon_md::{
    FsTrogonMdLoader, TrogonMdLayer, TrogonMdLoading, list_trogon_md_hierarchy, load_trogon_md, project_trogon_md_path,
};
pub use wasm_bash_tool::{BashOutputTool, WasmRuntimeBashTool};

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

/// Guidance appended to the system prompt so the model always reports back. With
/// only the bare identity, a model given tools tends to run them and end its turn
/// silently — the user sees tool calls happen and then nothing. This makes the
/// model close every turn with a short plain-language summary of what it did and
/// the result, the way Claude Code does, without producing a wall of text.
pub const COMPLETION_GUIDANCE: &str = "After you do real work — running tools or changing files — let the user know in natural, conversational language what you did and how it went, the way a helpful colleague would. Don't go silent right after a tool call: the user sees only what you say, not the raw tool output. When you did not run any tools or change anything (a greeting, a question you simply answered, ordinary back-and-forth), just reply naturally and add nothing extra — no recap. Keep it brief and human, and reach for a bullet list only when the work genuinely has several distinct parts worth itemizing, not by default.";

/// Injected as a user message when a turn ran tools but ended with no text — the
/// model did work and went silent. The hard backstop behind [`COMPLETION_GUIDANCE`]
/// (which is soft): each runner detects the silent-after-tools case, sends this
/// once, and loops once more to get the recap. Mirrors agent-core's own nudge.
pub const AUTO_SUMMARY_NUDGE: &str = "You ended your turn without telling the user what you did. In 1-2 sentences of natural language, briefly summarize what you just did and the result. Do not call any more tools.";
