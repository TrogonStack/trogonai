use async_nats::jetstream;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use trogon_tools::Message;

use crate::egress::EgressPolicy;
use crate::scope::Scope;

/// An MCP server configuration stored per session.
///
/// Two transports: HTTP/SSE (via `url`) and native stdio (via `command`). When
/// `command` is non-empty the runner spawns the server subprocess and drives it
/// in-process over stdio (no local HTTP bridge); otherwise `url` is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMcpServer {
    /// Human-readable name (used as tool prefix).
    pub name: String,
    /// HTTP or SSE endpoint URL (empty for stdio servers).
    #[serde(default)]
    pub url: String,
    /// Optional HTTP headers (name, value pairs).
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// For a stdio MCP server: the executable to spawn. When set, `url`/`headers`
    /// are ignored and the runner drives the server natively over stdio.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Command-line arguments for the stdio server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment variables (name, value) for the stdio server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
    /// Optional per-request timeout in seconds. `None` uses the runner's default
    /// HTTP client timeout. Carried from the client via the ACP `_meta` field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

// ── Feature 1: Path-scoped RBAC ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    RequireApproval,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolPolicy {
    pub tool: String,
    pub path_pattern: String,
    pub action: PolicyAction,
}

// ── Feature 3: Audit trail ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Allowed,
    Denied,
    RequiredApproval,
    ApprovedByUser,
    DeniedByUser,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEntry {
    pub timestamp: String,
    pub tool: String,
    pub input_summary: String,
    pub outcome: AuditOutcome,
}

const AUDIT_LOG_CAP: usize = 500;

/// A single todo item stored in the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    /// One of: `pending`, `in_progress`, `completed`.
    pub status: String,
}

const BUCKET: &str = "ACP_SESSIONS";

/// A background bash job started via `bash` with `run_in_background: true`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BashJob {
    pub id: String,
    pub command: String,
    pub terminal_id: String,
    pub start_marker: String,
    pub exit_marker_prefix: String,
    #[serde(default)]
    pub read_offset: usize,
    #[serde(default)]
    pub finished: bool,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// Persisted state for a single ACP session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub messages: Vec<Message>,
    /// Per-session model override. `None` means use the agent's default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Per-session context-compaction model override (same provider). `None`
    /// means compact with the session model. Set via `set_session_config_option`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compactor_model: Option<String>,
    /// Permission mode (e.g. "default", "acceptEdits", "bypassPermissions").
    #[serde(default)]
    pub mode: String,
    /// Working directory recorded when the session was created.
    #[serde(default)]
    pub cwd: String,
    /// ISO-8601 creation timestamp.
    #[serde(default)]
    pub created_at: String,
    /// ISO-8601 last-modified timestamp (updated on every save).
    #[serde(default)]
    pub updated_at: String,
    /// Short title derived from the first prompt (max 256 chars).
    #[serde(default)]
    pub title: String,
    /// Per-session MCP server configurations (HTTP/SSE only).
    #[serde(default)]
    pub mcp_servers: Vec<StoredMcpServer>,
    /// Per-session OpenAPI connections. Each spec operation becomes a callable
    /// tool (`{name}__{operationId}`) dispatched through the MCP path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openapi_servers: Vec<crate::openapi::StoredOpenApiServer>,
    /// Optional system prompt set at session creation via `_meta.systemPrompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Full override of the built-in identity, set at session creation via
    /// `_meta.systemPromptOverride` (`--system-prompt`). When set it replaces the
    /// default "You are Trogon" identity; TROGON.md and the appended system prompt
    /// are still applied on top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,
    /// Additional root directories supplied via `_meta.additionalRoots` at session creation.
    #[serde(default)]
    pub additional_roots: Vec<String>,
    /// Read-only allow-listed directories outside cwd, from
    /// `_meta.permissions.additionalDirectories`. Reads here are auto-approved even
    /// though they are outside the working directory.
    #[serde(default)]
    pub additional_read_dirs: Vec<String>,
    /// Lifecycle hooks for tool events (PreToolUse/PostToolUse), sent from the CLI
    /// via `_meta.toolHooks`. Run by the runner around tool dispatch.
    #[serde(default, skip_serializing_if = "crate::hooks::HooksConfig::is_empty")]
    pub tool_hooks: crate::hooks::HooksConfig,
    /// If true, all built-in agent tools are disabled for this session.
    /// Set via `_meta.disableBuiltInTools` at session creation.
    #[serde(default)]
    pub disable_builtin_tools: bool,
    /// Tools for which the user chose "Always Allow" — auto-approved on future calls.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Restrictive allowlist: when non-empty, only these tool names may be offered
    /// to the model (empty = no restriction, inherit all tools).
    #[serde(default)]
    pub tool_allowlist: Vec<String>,
    /// Session this was branched from. None for root sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// Message index at which the branch was made (exclusive upper bound).
    /// None means the entire source history was copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branched_at_index: Option<usize>,
    /// Structured per-tool path policies evaluated before the interactive gate.
    #[serde(default)]
    pub tool_policies: Vec<ToolPolicy>,
    /// Optional Scope envelope for the low-friction permission model (Phase 2).
    /// `None` falls back to the mode-based permission path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// Egress policy for outbound MCP server connections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress_policy: Option<EgressPolicy>,
    /// Audit log of permission decisions for this session.
    #[serde(default)]
    pub audit_log: Vec<AuditEntry>,
    /// ID of the persistent bash terminal created for this session.
    /// Set on the first bash call; subsequent calls reuse the same terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    /// Working directory the persistent terminal was created in.
    /// When this differs from the current session cwd, `terminal_id` is cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_cwd: Option<String>,
    /// Todo list for this session, persisted in NATS KV.
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    /// Permission rules text set via `/config` (same format as TROGON.md `## Permissions` section).
    /// Merged with rules loaded from TROGON.md — session rules take precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_rules_text: Option<String>,
    /// Spawn nesting depth: 0 for root sessions, 1 for direct sub-agents, etc.
    /// Used to enforce a maximum spawn depth and prevent runaway recursion.
    #[serde(default)]
    pub spawn_depth: u32,
    /// Token budget used to decide when to compact the message history.
    /// Compaction triggers at 85 % of this value. Default: 200 000.
    #[serde(default = "default_token_budget", skip_serializing_if = "is_default_token_budget")]
    pub token_budget: u64,
    /// Cumulative input tokens consumed across all prompt turns in this session.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub total_input_tokens: u64,
    /// Cumulative output tokens generated across all prompt turns in this session.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub total_output_tokens: u64,
    /// Cumulative cache-creation tokens across all prompt turns in this session.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub total_cache_creation_tokens: u64,
    /// Cumulative cache-read tokens across all prompt turns in this session.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub total_cache_read_tokens: u64,
    /// Background bash jobs started with `run_in_background: true`.
    #[serde(default)]
    pub background_jobs: Vec<BashJob>,
    /// Extra environment variables applied to the session's bash terminal, from
    /// settings.json `env` (received via `_meta.env` at session creation).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub env: std::collections::HashMap<String, String>,
}

/// Append new audit entries to the log, trimming oldest entries if over cap.
pub fn append_audit_entries(audit_log: &mut Vec<AuditEntry>, mut new_entries: Vec<AuditEntry>) {
    audit_log.append(&mut new_entries);
    if audit_log.len() > AUDIT_LOG_CAP {
        let excess = audit_log.len() - AUDIT_LOG_CAP;
        audit_log.drain(..excess);
    }
}

fn default_token_budget() -> u64 {
    200_000
}

fn is_default_token_budget(v: &u64) -> bool {
    *v == 200_000
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            model: None,
            compactor_model: None,
            mode: String::new(),
            cwd: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            title: String::new(),
            mcp_servers: Vec::new(),
            openapi_servers: Vec::new(),
            system_prompt: None,
            system_prompt_override: None,
            additional_roots: Vec::new(),
            additional_read_dirs: Vec::new(),
            tool_hooks: crate::hooks::HooksConfig::default(),
            disable_builtin_tools: false,
            allowed_tools: Vec::new(),
            tool_allowlist: Vec::new(),
            parent_session_id: None,
            branched_at_index: None,
            tool_policies: Vec::new(),
            scope: None,
            egress_policy: None,
            audit_log: Vec::new(),
            terminal_id: None,
            terminal_cwd: None,
            todos: Vec::new(),
            permission_rules_text: None,
            token_budget: default_token_budget(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            spawn_depth: 0,
            background_jobs: Vec::new(),
            env: HashMap::new(),
        }
    }
}

/// Returns true when `allowlist` is empty (no restriction) or `name` is listed.
pub fn is_tool_in_allowlist(allowlist: &[String], name: &str) -> bool {
    allowlist.is_empty() || allowlist.iter().any(|t| t == name)
}

/// Restrict `tools` to names in `allowlist` when the allowlist is non-empty.
pub fn filter_tool_defs_by_allowlist(
    tools: Vec<trogon_tools::ToolDef>,
    allowlist: &[String],
) -> Vec<trogon_tools::ToolDef> {
    if allowlist.is_empty() {
        return tools;
    }
    tools
        .into_iter()
        .filter(|d| allowlist.iter().any(|t| t == &d.name))
        .collect()
}

/// Per-turn tool allow-list from an ACP prompt `_meta.toolAllowlist` array.
/// When the meta field is absent or empty, returns `session_allowlist`.
pub fn turn_tool_allowlist_from_prompt_meta(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
    session_allowlist: Vec<String>,
) -> Vec<String> {
    let from_meta = meta
        .and_then(|m| m.get("toolAllowlist"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if from_meta.is_empty() {
        session_allowlist
    } else {
        from_meta
    }
}

/// Intersect `enabled` with `allowlist` when the allowlist is non-empty.
pub fn intersect_enabled_tools(enabled: &[String], allowlist: &[String]) -> Vec<String> {
    if allowlist.is_empty() {
        return enabled.to_vec();
    }
    enabled
        .iter()
        .filter(|t| allowlist.iter().any(|a| a == *t))
        .cloned()
        .collect()
}

// ── SessionStore trait ────────────────────────────────────────────────────────

/// Persistence interface for ACP session state.
///
/// The production implementation (`NatsSessionStore`) is backed by NATS KV.
/// Tests can use `MemorySessionStore` (behind the `test-helpers` feature) to
/// avoid any network dependencies.
#[async_trait]
pub trait SessionStore: Clone + Send + Sync + 'static {
    async fn load(&self, session_id: &str) -> anyhow::Result<SessionState>;
    async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()>;
    async fn delete(&self, session_id: &str) -> anyhow::Result<()>;
    async fn list_ids(&self) -> anyhow::Result<Vec<String>>;
    /// Return the IDs of all sessions whose `parent_session_id` matches `parent_id`.
    async fn list_children(&self, parent_id: &str) -> anyhow::Result<Vec<String>>;
}

// ── In-memory allowed-tools store (xai/openrouter permission bridge) ───────────

/// Minimal session store used by non-ACP runners for `allow_always` persistence.
#[derive(Clone, Default)]
pub struct AllowedToolsSessionStore {
    allowed: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

impl AllowedToolsSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allowed_tools(&self, session_id: &str) -> Vec<String> {
        self.allowed
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl SessionStore for AllowedToolsSessionStore {
    async fn load(&self, session_id: &str) -> anyhow::Result<SessionState> {
        Ok(SessionState {
            allowed_tools: self.allowed_tools(session_id),
            ..SessionState::default()
        })
    }

    async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
        self.allowed
            .lock()
            .unwrap()
            .insert(session_id.to_string(), state.allowed_tools.clone());
        Ok(())
    }

    async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
        self.allowed.lock().unwrap().remove(session_id);
        Ok(())
    }

    async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.allowed.lock().unwrap().keys().cloned().collect())
    }

    async fn list_children(&self, _parent_id: &str) -> anyhow::Result<Vec<String>> {
        Ok(vec![])
    }
}

// ── NATS KV implementation ────────────────────────────────────────────────────

/// NATS KV-backed session store.
#[derive(Clone)]
pub struct NatsSessionStore {
    kv: jetstream::kv::Store,
}

impl NatsSessionStore {
    /// Create or open the `ACP_SESSIONS` KV bucket.
    #[cfg_attr(coverage, coverage(off))]
    pub async fn open(js: &jetstream::Context) -> anyhow::Result<Self> {
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket: BUCKET.to_string(),
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl SessionStore for NatsSessionStore {
    /// Load session history, returning an empty state if the key does not exist.
    #[cfg_attr(coverage, coverage(off))]
    async fn load(&self, session_id: &str) -> anyhow::Result<SessionState> {
        match self.kv.get(session_id).await? {
            Some(bytes) => Ok(serde_json::from_slice(&bytes)?),
            None => Ok(SessionState::default()),
        }
    }

    /// Persist updated session state.
    #[cfg_attr(coverage, coverage(off))]
    async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(state)?;
        self.kv.put(session_id, bytes.into()).await?;
        Ok(())
    }

    /// Delete a session from the store (best-effort).
    #[cfg_attr(coverage, coverage(off))]
    async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
        self.kv.delete(session_id).await?;
        Ok(())
    }

    /// List all session IDs currently in the store.
    #[cfg_attr(coverage, coverage(off))]
    async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
        use futures_util::StreamExt;
        let mut keys = self.kv.keys().await?;
        let mut ids = Vec::new();
        while let Some(key) = keys.next().await {
            match key {
                Ok(k) => ids.push(k),
                Err(e) => tracing::warn!(error = %e, "session_store: error reading key"),
            }
        }
        Ok(ids)
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn list_children(&self, parent_id: &str) -> anyhow::Result<Vec<String>> {
        let ids = self.list_ids().await?;
        let mut children = Vec::new();
        for id in ids {
            if let Ok(state) = self.load(&id).await
                && state.parent_session_id.as_deref() == Some(parent_id)
            {
                children.push(id);
            }
        }
        Ok(children)
    }
}

// ── In-memory mock (test-helpers feature) ─────────────────────────────────────

#[cfg(any(test, feature = "test-helpers"))]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// In-memory `SessionStore` for unit tests (no NATS required).
    #[derive(Clone, Default)]
    pub struct MemorySessionStore {
        sessions: Arc<Mutex<HashMap<String, SessionState>>>,
    }

    impl MemorySessionStore {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for MemorySessionStore {
        async fn load(&self, session_id: &str) -> anyhow::Result<SessionState> {
            Ok(self
                .sessions
                .lock()
                .unwrap()
                .get(session_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
            self.sessions
                .lock()
                .unwrap()
                .insert(session_id.to_string(), state.clone());
            Ok(())
        }

        async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
            self.sessions.lock().unwrap().remove(session_id);
            Ok(())
        }

        async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
            Ok(self.sessions.lock().unwrap().keys().cloned().collect())
        }

        async fn list_children(&self, parent_id: &str) -> anyhow::Result<Vec<String>> {
            Ok(self
                .sessions
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, s)| s.parent_session_id.as_deref() == Some(parent_id))
                .map(|(id, _)| id.clone())
                .collect())
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns the current UTC time as an ISO-8601 string (seconds precision).
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs();
            let (y, mo, day, h, min, s) = epoch_to_parts(secs);
            format!("{y:04}-{mo:02}-{day:02}T{h:02}:{min:02}:{s:02}Z")
        })
        .unwrap_or_default()
}

fn epoch_to_parts(mut secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    secs /= 60;
    let min = secs % 60;
    secs /= 60;
    let h = secs % 24;
    secs /= 24;
    let mut days = secs;
    let mut year = 1970u64;
    loop {
        let dy = days_in_year(year);
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let mut month = 1u64;
    loop {
        let dm = days_in_month(year, month);
        if days < dm {
            break;
        }
        days -= dm;
        month += 1;
    }
    (year, month, days + 1, h, min, s)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}
fn days_in_year(y: u64) -> u64 {
    if is_leap(y) { 366 } else { 365 }
}
fn days_in_month(y: u64, m: u64) -> u64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_leap ───────────────────────────────────────────────────────────────

    #[test]
    fn is_leap_returns_true_for_divisible_by_4() {
        assert!(is_leap(2024));
    }

    #[test]
    fn is_leap_returns_false_for_century_non_400() {
        assert!(!is_leap(1900));
        assert!(!is_leap(2100));
    }

    #[test]
    fn is_leap_returns_true_for_400_multiple() {
        assert!(is_leap(2000));
        assert!(is_leap(2400));
    }

    #[test]
    fn is_leap_returns_false_for_regular_year() {
        assert!(!is_leap(2023));
        assert!(!is_leap(2025));
    }

    // ── days_in_month ─────────────────────────────────────────────────────────

    #[test]
    fn days_in_month_january_is_31() {
        assert_eq!(days_in_month(2024, 1), 31);
    }

    #[test]
    fn days_in_month_april_is_30() {
        assert_eq!(days_in_month(2024, 4), 30);
    }

    #[test]
    fn days_in_month_feb_leap_year_is_29() {
        assert_eq!(days_in_month(2024, 2), 29);
    }

    #[test]
    fn days_in_month_feb_non_leap_year_is_28() {
        assert_eq!(days_in_month(2023, 2), 28);
    }

    #[test]
    fn days_in_month_december_is_31() {
        assert_eq!(days_in_month(2024, 12), 31);
    }

    #[test]
    fn days_in_month_invalid_month_returns_fallback() {
        // Month 0 and 13 are not valid calendar months — the fallback `_ => 30` is hit.
        assert_eq!(days_in_month(2024, 0), 30);
        assert_eq!(days_in_month(2024, 13), 30);
    }

    // ── epoch_to_parts ────────────────────────────────────────────────────────

    #[test]
    fn epoch_to_parts_unix_epoch_zero() {
        // 0 seconds = 1970-01-01 00:00:00 UTC
        assert_eq!(epoch_to_parts(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_to_parts_known_timestamp() {
        // 2024-01-01T00:00:00Z = 1704067200
        assert_eq!(epoch_to_parts(1_704_067_200), (2024, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_to_parts_known_timestamp_with_time() {
        // 2024-03-19T12:34:56Z = 1710851696
        let (y, mo, d, h, min, s) = epoch_to_parts(1_710_851_696);
        assert_eq!(y, 2024);
        assert_eq!(mo, 3);
        assert_eq!(d, 19);
        assert_eq!(h, 12);
        assert_eq!(min, 34);
        assert_eq!(s, 56);
    }

    #[test]
    fn epoch_to_parts_end_of_1970() {
        // 1970-12-31T23:59:59Z
        let (y, mo, d, h, min, s) = epoch_to_parts(365 * 86400 - 1);
        assert_eq!(y, 1970);
        assert_eq!(mo, 12);
        assert_eq!(d, 31);
        assert_eq!(h, 23);
        assert_eq!(min, 59);
        assert_eq!(s, 59);
    }

    // ── now_iso8601 ───────────────────────────────────────────────────────────

    #[test]
    fn now_iso8601_has_correct_format() {
        let ts = now_iso8601();
        // Must match YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(ts.len(), 20, "unexpected length: {ts}");
        assert!(ts.ends_with('Z'), "must end with Z: {ts}");
        assert_eq!(&ts[4..5], "-", "separator after year: {ts}");
        assert_eq!(&ts[7..8], "-", "separator after month: {ts}");
        assert_eq!(&ts[10..11], "T", "T separator: {ts}");
        assert_eq!(&ts[13..14], ":", "colon after hour: {ts}");
        assert_eq!(&ts[16..17], ":", "colon after minute: {ts}");
    }

    // ── StoredMcpServer serde ─────────────────────────────────────────────────

    #[test]
    fn stored_mcp_server_roundtrip_with_headers() {
        let server = StoredMcpServer {
            name: "my-server".to_string(),
            url: "https://mcp.example.com/sse".to_string(),
            headers: vec![
                ("Authorization".to_string(), "Bearer tok".to_string()),
                ("X-Tenant".to_string(), "acme".to_string()),
            ],
            command: String::new(),
            args: vec![],
            env: vec![],
            timeout_secs: None,
        };
        let json = serde_json::to_string(&server).unwrap();
        let back: StoredMcpServer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "my-server");
        assert_eq!(back.url, "https://mcp.example.com/sse");
        assert_eq!(back.headers.len(), 2);
        assert_eq!(back.headers[0], ("Authorization".to_string(), "Bearer tok".to_string()));
    }

    #[test]
    fn stored_mcp_server_empty_headers_roundtrip() {
        let server = StoredMcpServer {
            name: "bare".to_string(),
            url: "http://localhost:8080".to_string(),
            headers: vec![],
            command: String::new(),
            args: vec![],
            env: vec![],
            timeout_secs: None,
        };
        let json = serde_json::to_string(&server).unwrap();
        let back: StoredMcpServer = serde_json::from_str(&json).unwrap();
        assert!(back.headers.is_empty());
    }

    #[test]
    fn stored_mcp_server_stdio_roundtrip() {
        let server = StoredMcpServer {
            name: "fs".to_string(),
            url: String::new(),
            headers: vec![],
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "server-filesystem".to_string()],
            env: vec![("TOKEN".to_string(), "x".to_string())],
            timeout_secs: Some(10),
        };
        let json = serde_json::to_string(&server).unwrap();
        let back: StoredMcpServer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.command, "npx");
        assert_eq!(back.args.len(), 2);
        assert_eq!(back.env[0], ("TOKEN".to_string(), "x".to_string()));
        assert!(back.url.is_empty());
    }

    #[test]
    fn stored_mcp_server_missing_headers_field_defaults_to_empty() {
        let json = r#"{"name":"s","url":"http://x"}"#;
        let server: StoredMcpServer = serde_json::from_str(json).unwrap();
        assert!(server.headers.is_empty(), "headers must default to empty vec");
    }

    // ── TodoItem serde ────────────────────────────────────────────────────────

    #[test]
    fn todo_item_roundtrip() {
        let item = TodoItem {
            id: "task-1".to_string(),
            content: "Write tests".to_string(),
            status: "in_progress".to_string(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "task-1");
        assert_eq!(back.content, "Write tests");
        assert_eq!(back.status, "in_progress");
    }

    #[test]
    fn todo_item_pending_status_roundtrip() {
        let item = TodoItem {
            id: "t".to_string(),
            content: "c".to_string(),
            status: "pending".to_string(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, "pending");
    }

    #[test]
    fn todo_item_completed_status_roundtrip() {
        let item = TodoItem {
            id: "t".to_string(),
            content: "c".to_string(),
            status: "completed".to_string(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, "completed");
    }

    #[test]
    fn todo_item_clone_is_independent() {
        let item = TodoItem {
            id: "orig".to_string(),
            content: "c".to_string(),
            status: "pending".to_string(),
        };
        let mut cloned = item.clone();
        cloned.id = "copy".to_string();
        assert_eq!(item.id, "orig", "clone must not mutate original");
    }

    #[test]
    fn now_iso8601_year_is_plausible() {
        let ts = now_iso8601();
        let year: u32 = ts[..4].parse().expect("year must be numeric");
        assert!(year >= 2024, "year {year} seems too early");
        assert!(year < 2100, "year {year} seems too far in the future");
    }

    // ── SessionState serde ────────────────────────────────────────────────────

    #[test]
    fn session_state_default_roundtrip() {
        let state = SessionState::default();
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mode, "");
        assert!(back.messages.is_empty());
    }

    #[test]
    fn session_state_optional_fields_omitted_from_json() {
        let state = SessionState {
            model: None,
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("\"model\""), "None model must be omitted: {json}");
    }

    #[test]
    fn session_state_model_present_when_set() {
        let state = SessionState {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("claude-opus-4-6"), "model must be serialized: {json}");
    }

    #[test]
    fn session_state_scope_round_trips() {
        let scope = Scope::baseline("/repo");
        let state = SessionState {
            scope: Some(scope.clone()),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.scope, Some(scope));
    }

    #[test]
    fn session_state_scope_none_omitted_from_json() {
        let json = serde_json::to_string(&SessionState::default()).unwrap();
        assert!(!json.contains("\"scope\""), "None scope must be omitted: {json}");
    }

    // ── branching fields ──────────────────────────────────────────────────────

    #[test]
    fn session_state_branching_fields_omitted_when_none() {
        let state = SessionState::default();
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("parent_session_id"),
            "parent_session_id must be omitted when None: {json}"
        );
        assert!(
            !json.contains("branched_at_index"),
            "branched_at_index must be omitted when None: {json}"
        );
    }

    #[test]
    fn session_state_branching_fields_roundtrip() {
        let state = SessionState {
            parent_session_id: Some("root-session".to_string()),
            branched_at_index: Some(5),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.parent_session_id.as_deref(), Some("root-session"));
        assert_eq!(back.branched_at_index, Some(5));
    }

    // ── append_audit_entries ──────────────────────────────────────────────────

    fn make_entry(tool: &str, outcome: AuditOutcome) -> AuditEntry {
        AuditEntry {
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            tool: tool.to_string(),
            input_summary: tool.to_string(),
            outcome,
        }
    }

    #[test]
    fn append_audit_entries_to_empty_log() {
        let mut log = vec![];
        let new = vec![make_entry("Read", AuditOutcome::Allowed)];
        append_audit_entries(&mut log, new);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].tool, "Read");
    }

    #[test]
    fn append_audit_entries_to_existing_log() {
        let mut log = vec![make_entry("Read", AuditOutcome::Allowed)];
        let new = vec![make_entry("Write", AuditOutcome::ApprovedByUser)];
        append_audit_entries(&mut log, new);
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].tool, "Write");
    }

    #[test]
    fn append_audit_entries_empty_new_is_noop() {
        let mut log = vec![make_entry("Read", AuditOutcome::Allowed)];
        append_audit_entries(&mut log, vec![]);
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn append_audit_entries_exactly_at_cap_does_not_drop() {
        let mut log: Vec<AuditEntry> = (0..490)
            .map(|i| make_entry(&format!("tool-{i}"), AuditOutcome::Allowed))
            .collect();
        let new: Vec<AuditEntry> = (0..10)
            .map(|i| make_entry(&format!("new-{i}"), AuditOutcome::Allowed))
            .collect();
        append_audit_entries(&mut log, new);
        assert_eq!(log.len(), 500);
        assert_eq!(log[0].tool, "tool-0");
    }

    #[test]
    fn append_audit_entries_over_cap_drops_oldest() {
        let mut log: Vec<AuditEntry> = (0..495)
            .map(|i| make_entry(&format!("old-{i}"), AuditOutcome::Allowed))
            .collect();
        let new: Vec<AuditEntry> = (0..10)
            .map(|i| make_entry(&format!("new-{i}"), AuditOutcome::Allowed))
            .collect();
        append_audit_entries(&mut log, new);
        assert_eq!(log.len(), 500);
        assert_eq!(log[0].tool, "old-5", "oldest 5 entries must be dropped");
        assert_eq!(log[499].tool, "new-9");
    }

    // ── new SessionState fields serde ─────────────────────────────────────────

    #[test]
    fn tool_policies_default_empty_and_roundtrip() {
        let state = SessionState::default();
        assert!(state.tool_policies.is_empty());
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert!(back.tool_policies.is_empty());
    }

    #[test]
    fn tool_policies_roundtrip_with_entries() {
        use crate::session_store::PolicyAction;
        let state = SessionState {
            tool_policies: vec![ToolPolicy {
                tool: "write_file".to_string(),
                path_pattern: "/workspace/**".to_string(),
                action: PolicyAction::Allow,
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_policies.len(), 1);
        assert_eq!(back.tool_policies[0].tool, "write_file");
        assert_eq!(back.tool_policies[0].path_pattern, "/workspace/**");
        assert!(matches!(back.tool_policies[0].action, PolicyAction::Allow));
    }

    #[test]
    fn egress_policy_omitted_when_none() {
        let state = SessionState::default();
        assert!(state.egress_policy.is_none());
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("egress_policy"), "must be omitted when None: {json}");
    }

    #[test]
    fn egress_policy_roundtrip_when_set() {
        use crate::egress::{EgressAction, EgressRule};
        let state = SessionState {
            egress_policy: Some(EgressPolicy {
                default_action: EgressAction::Deny,
                rules: vec![EgressRule {
                    host_pattern: "api.anthropic.com".to_string(),
                    action: EgressAction::Allow,
                }],
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        let policy = back.egress_policy.unwrap();
        assert!(matches!(policy.default_action, EgressAction::Deny));
        assert_eq!(policy.rules.len(), 1);
        assert_eq!(policy.rules[0].host_pattern, "api.anthropic.com");
    }

    #[test]
    fn audit_log_default_empty_and_roundtrip() {
        let state = SessionState::default();
        assert!(state.audit_log.is_empty());
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert!(back.audit_log.is_empty());
    }

    #[test]
    fn audit_log_roundtrip_with_entries() {
        let state = SessionState {
            audit_log: vec![AuditEntry {
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                tool: "Read".to_string(),
                input_summary: "/etc/hosts".to_string(),
                outcome: AuditOutcome::Allowed,
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.audit_log.len(), 1);
        assert_eq!(back.audit_log[0].tool, "Read");
        assert_eq!(back.audit_log[0].outcome, AuditOutcome::Allowed);
    }

    // ── token_budget serde ────────────────────────────────────────────────────

    #[test]
    fn token_budget_default_is_200_000() {
        let state = SessionState::default();
        assert_eq!(state.token_budget, 200_000);
    }

    #[test]
    fn token_budget_default_omitted_from_json() {
        let state = SessionState::default();
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("token_budget"),
            "default token_budget must be omitted from JSON: {json}"
        );
    }

    #[test]
    fn token_budget_custom_value_serialized() {
        let state = SessionState {
            token_budget: 50_000,
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            json.contains("\"token_budget\":50000"),
            "custom token_budget must appear in JSON: {json}"
        );
    }

    #[test]
    fn token_budget_missing_from_json_deserializes_to_default() {
        let json = r#"{"messages":[],"mode":""}"#;
        let state: SessionState = serde_json::from_str(json).unwrap();
        assert_eq!(state.token_budget, 200_000);
    }

    #[test]
    fn token_budget_custom_value_roundtrip() {
        let state = SessionState {
            token_budget: 100_000,
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.token_budget, 100_000);
    }

    // ── Token tracking serialization ──────────────────────────────────────────

    #[test]
    fn session_state_zero_tokens_omitted_from_json() {
        let state = SessionState::default();
        let v: serde_json::Value = serde_json::to_value(&state).unwrap();
        assert!(
            v.get("total_input_tokens").is_none(),
            "zero total_input_tokens must be omitted from JSON"
        );
        assert!(
            v.get("total_output_tokens").is_none(),
            "zero total_output_tokens must be omitted from JSON"
        );
        assert!(
            v.get("total_cache_creation_tokens").is_none(),
            "zero total_cache_creation_tokens must be omitted from JSON"
        );
        assert!(
            v.get("total_cache_read_tokens").is_none(),
            "zero total_cache_read_tokens must be omitted from JSON"
        );
    }

    #[test]
    fn session_state_nonzero_tokens_round_trip() {
        let state = SessionState {
            total_input_tokens: 1000,
            total_output_tokens: 500,
            total_cache_creation_tokens: 200,
            total_cache_read_tokens: 75,
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_input_tokens, 1000);
        assert_eq!(back.total_output_tokens, 500);
        assert_eq!(back.total_cache_creation_tokens, 200);
        assert_eq!(back.total_cache_read_tokens, 75);
    }

    #[test]
    fn session_state_partial_nonzero_tokens_only_nonzero_fields_appear() {
        let state = SessionState {
            total_input_tokens: 42,
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::to_value(&state).unwrap();
        assert_eq!(v["total_input_tokens"], 42);
        assert!(v.get("total_output_tokens").is_none());
        assert!(v.get("total_cache_creation_tokens").is_none());
        assert!(v.get("total_cache_read_tokens").is_none());
    }

    // ── MemorySessionStore::list_children ─────────────────────────────────────

    #[cfg(feature = "test-helpers")]
    mod list_children_tests {
        use super::*;
        use mock::MemorySessionStore;

        #[tokio::test]
        async fn list_children_returns_direct_children() {
            let store = MemorySessionStore::new();
            store
                .save(
                    "child-1",
                    &SessionState {
                        parent_session_id: Some("root".to_string()),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            store
                .save(
                    "child-2",
                    &SessionState {
                        parent_session_id: Some("root".to_string()),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            store.save("unrelated", &SessionState::default()).await.unwrap();

            let mut children = store.list_children("root").await.unwrap();
            children.sort();
            assert_eq!(children, vec!["child-1", "child-2"]);
        }

        #[tokio::test]
        async fn list_children_returns_empty_for_unknown_parent() {
            let store = MemorySessionStore::new();
            store.save("s1", &SessionState::default()).await.unwrap();
            let children = store.list_children("no-such-parent").await.unwrap();
            assert!(children.is_empty());
        }

        #[tokio::test]
        async fn list_children_ignores_root_sessions() {
            let store = MemorySessionStore::new();
            store
                .save(
                    "root-session",
                    &SessionState {
                        parent_session_id: None,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let children = store.list_children("root-session").await.unwrap();
            assert!(children.is_empty());
        }
    }

    #[test]
    fn filter_tool_defs_by_allowlist_empty_means_no_restriction() {
        let defs = trogon_tools::all_tool_defs();
        let len = defs.len();
        let filtered = filter_tool_defs_by_allowlist(defs, &[]);
        assert_eq!(filtered.len(), len);
    }

    #[test]
    fn filter_tool_defs_by_allowlist_keeps_only_listed_names() {
        let defs = trogon_tools::all_tool_defs();
        let filtered = filter_tool_defs_by_allowlist(defs, &["read_file".to_string()]);
        assert!(filtered.iter().any(|d| d.name == "read_file"));
        assert!(!filtered.iter().any(|d| d.name == "write_file"));
    }

    #[test]
    fn turn_tool_allowlist_from_prompt_meta_prefers_non_empty_meta() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "toolAllowlist".into(),
            serde_json::json!(["read_file", "bash"]),
        );
        let out = turn_tool_allowlist_from_prompt_meta(Some(&meta), vec!["write_file".into()]);
        assert_eq!(out, vec!["read_file", "bash"]);
    }

    #[test]
    fn turn_tool_allowlist_from_prompt_meta_falls_back_to_session() {
        let out = turn_tool_allowlist_from_prompt_meta(None, vec!["read_file".into()]);
        assert_eq!(out, vec!["read_file"]);
        let out = turn_tool_allowlist_from_prompt_meta(Some(&serde_json::Map::new()), vec!["bash".into()]);
        assert_eq!(out, vec!["bash"]);
    }

    #[test]
    fn intersect_enabled_tools_applies_allowlist() {
        let enabled = vec![
            "read_file".to_string(),
            "write_file".to_string(),
            "web_search".to_string(),
        ];
        let out = intersect_enabled_tools(&enabled, &["read_file".to_string()]);
        assert_eq!(out, vec!["read_file".to_string()]);
    }
}
