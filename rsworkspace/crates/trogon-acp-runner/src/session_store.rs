use async_nats::jetstream;
use serde::{Deserialize, Serialize};
use trogon_agent_core::agent_loop::Message;

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
    #[cfg_attr(coverage, coverage(off))]
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
}
