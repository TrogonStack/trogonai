use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, kv};
use serde::Serialize;
use tracing::warn;

const SESSIONS_BUCKET: &str = "SESSIONS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Wire format for a session stored in the SESSIONS KV bucket.
/// Schema matches `ChatSession` from trogon-agent so trogon-console can read it.
#[derive(Clone, Serialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_path: Option<String>,
    pub messages: Vec<SnapshotMessage>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branched_at_index: Option<usize>,
}

/// Per-message token usage written to the SESSIONS bucket.
/// Field names match `RawUsage` in trogon-console so the console can sum totals.
#[derive(Clone, Serialize)]
pub struct MessageUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub cache_creation_input_tokens: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub cache_read_input_tokens: u32,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// A message entry in the snapshot — `content` uses the same tagged-block format
/// as `ContentBlock::Text` in trogon-agent so the console can render message text.
#[derive(Clone, Serialize)]
pub struct SnapshotMessage {
    pub role: String,
    pub content: Vec<TextBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<MessageUsage>,
}

/// A plain-text content block (`{"type":"text","text":"..."}`).
#[derive(Clone, Serialize)]
pub struct TextBlock {
    #[serde(rename = "type")]
    pub kind: &'static str, // always "text"
    pub text: String,
}

impl TextBlock {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            kind: "text",
            text: text.into(),
        }
    }
}

// ── Trait ─────────────────────────────────────────────────────────────────────

pub trait SessionStoring: Send + Sync + 'static {
    fn save<'a>(&'a self, snapshot: &'a SessionSnapshot) -> BoxFuture<'a, ()>;
    fn remove<'a>(&'a self, tenant_id: &'a str, session_id: &'a str) -> BoxFuture<'a, ()>;
}

// ── Real implementation ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct NatsSessionStore {
    sessions_kv: kv::Store,
}

impl NatsSessionStore {
    pub async fn open(js: &jetstream::Context, session_ttl_secs: u64) -> Result<Self, String> {
        let sessions_kv = js
            .create_or_update_key_value(kv::Config {
                bucket: SESSIONS_BUCKET.to_string(),
                history: 1,
                max_age: std::time::Duration::from_secs(session_ttl_secs),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self { sessions_kv })
    }

    async fn save_impl(&self, snapshot: &SessionSnapshot) {
        let key = format!("{}.{}", snapshot.tenant_id, snapshot.id);
        let bytes = match serde_json::to_vec(snapshot) {
            Ok(b) => b,
            Err(e) => {
                warn!(session_id = %snapshot.id, error = %e, "failed to serialize session snapshot");
                return;
            }
        };
        if let Err(e) = self.sessions_kv.put(&key, bytes.into()).await {
            warn!(session_id = %snapshot.id, error = %e, "failed to write session to SESSIONS KV");
        }
    }

    async fn remove_impl(&self, tenant_id: &str, session_id: &str) {
        let key = format!("{tenant_id}.{session_id}");
        if let Err(e) = self.sessions_kv.delete(&key).await {
            warn!(session_id, error = %e, "failed to delete session from SESSIONS KV");
        }
    }
}

impl SessionStoring for NatsSessionStore {
    fn save<'a>(&'a self, snapshot: &'a SessionSnapshot) -> BoxFuture<'a, ()> {
        Box::pin(self.save_impl(snapshot))
    }

    fn remove<'a>(&'a self, tenant_id: &'a str, session_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(self.remove_impl(tenant_id, session_id))
    }
}

// ── Timestamp helper ──────────────────────────────────────────────────────────

/// Returns the current UTC time as an ISO 8601 string (e.g. `2026-04-20T15:30:00.123Z`).
pub fn now_iso() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let millis = d.subsec_millis();
    let time_secs = secs % 86400;
    let h = time_secs / 3600;
    let min = (time_secs % 3600) / 60;
    let s = time_secs % 60;
    let (y, mo, day) = days_to_ymd(secs / 86400);
    format!("{y:04}-{mo:02}-{day:02}T{h:02}:{min:02}:{s:02}.{millis:03}Z")
}

/// Converts a count of days since the Unix epoch (1970-01-01) to (year, month, day).
/// Uses the algorithm from https://howardhinnant.github.io/date_algorithms.html.
fn days_to_ymd(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::{BoxFuture, SessionSnapshot, SessionStoring};
    use std::sync::Mutex;

    pub struct MockSessionStore {
        pub saves: Mutex<Vec<SessionSnapshot>>,
        pub removes: Mutex<Vec<(String, String)>>,
    }

    impl MockSessionStore {
        pub fn new() -> Self {
            Self {
                saves: Mutex::new(vec![]),
                removes: Mutex::new(vec![]),
            }
        }
    }

    impl SessionStoring for MockSessionStore {
        fn save<'a>(&'a self, snapshot: &'a SessionSnapshot) -> BoxFuture<'a, ()> {
            self.saves
                .lock()
                .expect("saves lock poisoned")
                .push(snapshot.clone());
            Box::pin(std::future::ready(()))
        }

        fn remove<'a>(&'a self, tenant_id: &'a str, session_id: &'a str) -> BoxFuture<'a, ()> {
            self.removes
                .lock()
                .expect("removes lock poisoned")
                .push((tenant_id.to_string(), session_id.to_string()));
            Box::pin(std::future::ready(()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_serializes_to_chat_session_schema() {
        let snap = SessionSnapshot {
            id: "s1".into(),
            tenant_id: "acme".into(),
            name: "Hello world".into(),
            model: Some("grok-4".into()),
            tools: vec![],
            memory_path: None,
            agent_id: Some("agent-42".into()),
            messages: vec![
                SnapshotMessage {
                    role: "user".into(),
                    content: vec![TextBlock::new("Hello")],
                    usage: None,
                },
                SnapshotMessage {
                    role: "assistant".into(),
                    content: vec![TextBlock::new("Hi there!")],
                    usage: Some(MessageUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    }),
                },
            ],
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            parent_session_id: None,
            branched_at_index: None,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["id"], "s1");
        assert_eq!(v["model"], "grok-4");
        assert_eq!(v["agent_id"], "agent-42");
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"][0]["type"], "text");
        assert_eq!(v["messages"][0]["content"][0]["text"], "Hello");
        assert!(v["messages"][0].get("usage").is_none()); // skip_serializing_if None
        assert_eq!(v["messages"][1]["usage"]["input_tokens"], 10);
        assert_eq!(v["messages"][1]["usage"]["output_tokens"], 5);
        assert!(!json.contains("memory_path")); // skip_serializing_if None
    }

    #[test]
    fn now_iso_looks_like_iso8601() {
        let s = now_iso();
        assert_eq!(s.len(), 24);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], "T");
        assert!(s.ends_with('Z'));
    }

    #[test]
    fn days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2000-01-01: 30*365 + 7 leap years in [1972..=1996] = 10950 + 7 = 10957
        assert_eq!(days_to_ymd(10957), (2000, 1, 1));
    }
}
