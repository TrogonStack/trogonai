use async_nats::jetstream;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use trogon_tools::Message;

use crate::egress::EgressPolicy;

/// A URL-based MCP server configuration stored per session.
/// Stdio servers are not supported in the NATS model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMcpServer {
    /// Human-readable name (used as tool prefix).
    pub name: String,
    /// HTTP or SSE endpoint URL.
    pub url: String,
    /// Optional HTTP headers (name, value pairs).
    #[serde(default)]
    pub headers: Vec<(String, String)>,
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

/// Persisted state for a single ACP session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub messages: Vec<Message>,
    /// Per-session model override. `None` means use the agent's default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    /// Optional system prompt set at session creation via `_meta.systemPrompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Additional root directories supplied via `_meta.additionalRoots` at session creation.
    #[serde(default)]
    pub additional_roots: Vec<String>,
    /// If true, all built-in agent tools are disabled for this session.
    /// Set via `_meta.disableBuiltInTools` at session creation.
    #[serde(default)]
    pub disable_builtin_tools: bool,
    /// Tools for which the user chose "Always Allow" — auto-approved on future calls.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
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
    /// Todo list for this session, persisted in NATS KV.
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    /// Permission rules text set via `/config` (same format as TROGON.md `## Permissions` section).
    /// Merged with rules loaded from TROGON.md — session rules take precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_rules_text: Option<String>,
    /// Token budget used to decide when to compact the message history.
    /// Compaction triggers at 85 % of this value. Default: 200 000.
    #[serde(default = "default_token_budget", skip_serializing_if = "is_default_token_budget")]
    pub token_budget: u64,
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

impl Default for SessionState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            model: None,
            mode: String::new(),
            cwd: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            title: String::new(),
            mcp_servers: Vec::new(),
            system_prompt: None,
            additional_roots: Vec::new(),
            disable_builtin_tools: false,
            allowed_tools: Vec::new(),
            parent_session_id: None,
            branched_at_index: None,
            tool_policies: Vec::new(),
            egress_policy: None,
            audit_log: Vec::new(),
            terminal_id: None,
            todos: Vec::new(),
            permission_rules_text: None,
            token_budget: default_token_budget(),
        }
    }
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
        };
        let json = serde_json::to_string(&server).unwrap();
        let back: StoredMcpServer = serde_json::from_str(&json).unwrap();
        assert!(back.headers.is_empty());
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
        assert!(
            !json.contains("\"model\""),
            "None model must be omitted: {json}"
        );
    }

    #[test]
    fn session_state_model_present_when_set() {
        let state = SessionState {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            json.contains("claude-opus-4-6"),
            "model must be serialized: {json}"
        );
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
}
