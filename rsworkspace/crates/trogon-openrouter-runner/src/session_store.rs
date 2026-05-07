use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, kv};
use serde::Serialize;
use tracing::warn;

const SESSIONS_BUCKET: &str = "SESSIONS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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

#[derive(Clone, Serialize)]
pub struct SnapshotMessage {
    pub role: String,
    pub content: Vec<TextBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<MessageUsage>,
}

#[derive(Clone, Serialize)]
pub struct TextBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
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

pub(crate) fn days_to_ymd(days: u64) -> (i64, u32, u32) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── days_to_ymd ───────────────────────────────────────────────────────────

    #[test]
    fn days_to_ymd_unix_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_dates() {
        assert_eq!(days_to_ymd(1), (1970, 1, 2));
        assert_eq!(days_to_ymd(365), (1971, 1, 1));
        // 2021-01-01: 51 years × 365 + 13 leap days = 18628
        assert_eq!(days_to_ymd(18628), (2021, 1, 1));
        // 2024-01-01: confirmed by algorithm
        assert_eq!(days_to_ymd(19723), (2024, 1, 1));
        // 2023-12-31: one day before 2024-01-01
        assert_eq!(days_to_ymd(19722), (2023, 12, 31));
    }

    #[test]
    fn days_to_ymd_leap_year_feb29() {
        // 2024-01-01 = day 19723; +31 (Jan) +28 (Feb 1-28) = 19782 = Feb 29
        assert_eq!(days_to_ymd(19782), (2024, 2, 29));
        // Following day must be Mar 1
        assert_eq!(days_to_ymd(19783), (2024, 3, 1));
    }

    #[test]
    fn days_to_ymd_end_of_year() {
        assert_eq!(days_to_ymd(19722), (2023, 12, 31));
        assert_eq!(days_to_ymd(19723), (2024, 1, 1));
    }

    #[test]
    fn days_to_ymd_century_non_leap() {
        // 1900 is NOT a leap year (div by 100 but not 400).
        // 1900-03-01: days before epoch are negative; test via a positive known date.
        // 2100-03-01 is not a leap year either, but we can test known post-epoch dates.
        // 2000-02-29 IS valid (2000 is div by 400 → leap).
        // Days to 2000-01-01: 30 years + 8 leap days = 10950 + 8 - 1 = actually
        // 2000-01-01 is day 10957 since epoch.
        assert_eq!(days_to_ymd(10957), (2000, 1, 1));
        // 2000-02-29 = 10957 + 59 = 11016
        assert_eq!(days_to_ymd(11016), (2000, 2, 29));
    }

    // ── now_iso ───────────────────────────────────────────────────────────────

    #[test]
    fn now_iso_has_correct_format() {
        let s = now_iso();
        // YYYY-MM-DDTHH:MM:SS.MMMZ
        assert_eq!(s.len(), 24, "expected 24 chars, got {s:?}");
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], "T");
        assert_eq!(&s[13..14], ":");
        assert_eq!(&s[16..17], ":");
        assert_eq!(&s[19..20], ".");
        assert_eq!(&s[23..24], "Z");
    }

    #[test]
    fn now_iso_is_monotonically_non_decreasing() {
        let a = now_iso();
        let b = now_iso();
        assert!(b >= a, "now_iso should not go backward: a={a} b={b}");
    }

    // ── TextBlock ─────────────────────────────────────────────────────────────

    #[test]
    fn text_block_type_is_always_text() {
        let b = TextBlock::new("hello");
        assert_eq!(b.kind, "text");
        assert_eq!(b.text, "hello");
    }

    #[test]
    fn text_block_serializes_type_field() {
        let b = TextBlock::new("world");
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "world");
    }

    // ── SessionSnapshot serialization ─────────────────────────────────────────

    #[test]
    fn message_usage_zero_cache_fields_are_omitted() {
        let usage = MessageUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let v = serde_json::to_value(&usage).unwrap();
        assert!(v.get("cache_creation_input_tokens").is_none());
        assert!(v.get("cache_read_input_tokens").is_none());
        assert_eq!(v["input_tokens"], 10);
        assert_eq!(v["output_tokens"], 5);
    }

    #[test]
    fn message_usage_nonzero_cache_fields_are_included() {
        let usage = MessageUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_input_tokens: 2,
            cache_read_input_tokens: 3,
        };
        let v = serde_json::to_value(&usage).unwrap();
        assert_eq!(v["cache_creation_input_tokens"], 2);
        assert_eq!(v["cache_read_input_tokens"], 3);
    }

    #[test]
    fn snapshot_optional_fields_are_omitted_when_none() {
        let snap = SessionSnapshot {
            id: "s1".to_string(),
            tenant_id: "t1".to_string(),
            name: "Test".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            messages: vec![],
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            updated_at: "2026-01-01T00:00:00.000Z".to_string(),
            agent_id: None,
            parent_session_id: None,
            branched_at_index: None,
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert!(v.get("model").is_none());
        assert!(v.get("memory_path").is_none());
        assert!(v.get("agent_id").is_none());
        assert!(v.get("parent_session_id").is_none());
        assert!(v.get("branched_at_index").is_none());
    }

    #[test]
    fn snapshot_optional_fields_are_present_when_set() {
        let snap = SessionSnapshot {
            id: "s1".to_string(),
            tenant_id: "t1".to_string(),
            name: "Test".to_string(),
            model: Some("gpt-4".to_string()),
            tools: vec![],
            memory_path: Some("/tmp".to_string()),
            messages: vec![],
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            updated_at: "2026-01-01T00:00:00.000Z".to_string(),
            agent_id: Some("agent-1".to_string()),
            parent_session_id: Some("parent-id".to_string()),
            branched_at_index: Some(3),
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["model"], "gpt-4");
        assert_eq!(v["memory_path"], "/tmp");
        assert_eq!(v["agent_id"], "agent-1");
        assert_eq!(v["parent_session_id"], "parent-id");
        assert_eq!(v["branched_at_index"], 3);
    }

    #[test]
    fn snapshot_tools_field_always_serializes() {
        // tools: vec![] must serialize as an explicit empty array (not omitted).
        let snap = SessionSnapshot {
            id: "s".to_string(),
            tenant_id: "t".to_string(),
            name: "N".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            messages: vec![],
            created_at: "x".to_string(),
            updated_at: "y".to_string(),
            agent_id: None,
            parent_session_id: None,
            branched_at_index: None,
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert!(v["tools"].is_array(), "tools must always be serialized");
        assert_eq!(v["tools"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn snapshot_with_messages_serializes_correctly() {
        let snap = SessionSnapshot {
            id: "s".to_string(),
            tenant_id: "t".to_string(),
            name: "N".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            messages: vec![SnapshotMessage {
                role: "user".to_string(),
                content: vec![TextBlock::new("hi")],
                usage: Some(MessageUsage {
                    input_tokens: 5,
                    output_tokens: 3,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            }],
            created_at: "x".to_string(),
            updated_at: "y".to_string(),
            agent_id: None,
            parent_session_id: None,
            branched_at_index: None,
        };
        let v = serde_json::to_value(&snap).unwrap();
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"][0]["text"], "hi");
        assert_eq!(msgs[0]["content"][0]["type"], "text");
        assert_eq!(msgs[0]["usage"]["input_tokens"], 5);
        assert_eq!(msgs[0]["usage"]["output_tokens"], 3);
    }

    #[test]
    fn text_block_with_empty_content() {
        let b = TextBlock::new("");
        assert_eq!(b.kind, "text");
        assert_eq!(b.text, "");
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "");
    }

    #[test]
    fn message_usage_all_zero_cache_fields_are_omitted() {
        let usage = MessageUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let v = serde_json::to_value(&usage).unwrap();
        assert_eq!(v["input_tokens"], 0);
        assert_eq!(v["output_tokens"], 0);
        assert!(v.get("cache_creation_input_tokens").is_none());
        assert!(v.get("cache_read_input_tokens").is_none());
    }
}
