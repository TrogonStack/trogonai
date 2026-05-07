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
