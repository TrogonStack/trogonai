use async_nats::jetstream;
use serde::{Deserialize, Serialize};
use trogon_agent::agent_loop::Message;

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

const BUCKET: &str = "ACP_SESSIONS";

/// Persisted state for a single ACP session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
}

/// NATS KV-backed session store.
#[derive(Clone)]
pub struct SessionStore {
    kv: jetstream::kv::Store,
}

impl SessionStore {
    /// Create or open the `ACP_SESSIONS` KV bucket.
    pub async fn open(js: &jetstream::Context) -> anyhow::Result<Self> {
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket: BUCKET.to_string(),
                ..Default::default()
            })
            .await?;
        Ok(Self { kv })
    }

    /// Load session history, returning an empty state if the key does not exist.
    pub async fn load(&self, session_id: &str) -> anyhow::Result<SessionState> {
        match self.kv.get(session_id).await? {
            Some(bytes) => Ok(serde_json::from_slice(&bytes)?),
            None => Ok(SessionState::default()),
        }
    }

    /// Persist updated session state.
    pub async fn save(&self, session_id: &str, state: &SessionState) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(state)?;
        self.kv.put(session_id, bytes.into()).await?;
        Ok(())
    }

    /// Delete a session from the store (best-effort).
    pub async fn delete(&self, session_id: &str) -> anyhow::Result<()> {
        self.kv.delete(session_id).await?;
        Ok(())
    }

    /// List all session IDs currently in the store.
    pub async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
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
}

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
    let s = secs % 60; secs /= 60;
    let min = secs % 60; secs /= 60;
    let h = secs % 24; secs /= 24;
    let mut days = secs;
    let mut year = 1970u64;
    loop {
        let dy = days_in_year(year);
        if days < dy { break; }
        days -= dy; year += 1;
    }
    let mut month = 1u64;
    loop {
        let dm = days_in_month(year, month);
        if days < dm { break; }
        days -= dm; month += 1;
    }
    (year, month, days + 1, h, min, s)
}

fn is_leap(y: u64) -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 }
fn days_in_year(y: u64) -> u64 { if is_leap(y) { 366 } else { 365 } }
fn days_in_month(y: u64, m: u64) -> u64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap(y) { 29 } else { 28 },
        _ => 30,
    }
}
