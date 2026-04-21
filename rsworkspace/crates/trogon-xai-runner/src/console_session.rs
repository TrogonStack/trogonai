use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, kv};
use serde::Serialize;
use tracing::warn;

const SESSIONS_BUCKET: &str = "SESSIONS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ── Wire format ───────────────────────────────────────────────────────────────

/// Format written to the SESSIONS KV bucket; mirrors what trogon-console reads.
#[derive(Serialize)]
pub struct RawSession {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub messages: Vec<RawMessage>,
    pub created_at: String,
    pub updated_at: String,
    pub duration_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Serialize)]
pub struct RawMessage {
    pub role: String,
    pub content: String,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

pub trait ConsoleSessionWriting: Send + Sync + 'static {
    fn write<'a>(&'a self, key: &'a str, session: &'a RawSession) -> BoxFuture<'a, ()>;
}

// ── NATS KV implementation ────────────────────────────────────────────────────

pub struct KvConsoleSessionWriter {
    sessions_kv: kv::Store,
}

impl KvConsoleSessionWriter {
    pub async fn open(js: &jetstream::Context) -> Result<Self, String> {
        // Prefer the existing bucket config; create only if absent.
        let sessions_kv = match js.get_key_value(SESSIONS_BUCKET).await {
            Ok(store) => store,
            Err(_) => js
                .create_key_value(kv::Config {
                    bucket: SESSIONS_BUCKET.to_string(),
                    history: 1,
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?,
        };
        Ok(Self { sessions_kv })
    }
}

impl ConsoleSessionWriting for KvConsoleSessionWriter {
    fn write<'a>(&'a self, key: &'a str, session: &'a RawSession) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            match serde_json::to_vec(session) {
                Ok(bytes) => {
                    if let Err(e) = self.sessions_kv.put(key, bytes.into()).await {
                        warn!(error = %e, key, "xai: failed to write console session");
                    }
                }
                Err(e) => warn!(error = %e, "xai: failed to serialize console session"),
            }
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Format a Unix timestamp as an ISO 8601 UTC string (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn secs_to_iso8601(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let t = secs % 86400;
    let (y, mo, d) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, mo, d,
        t / 3600,
        (t % 3600) / 60,
        t % 60
    )
}

/// Howard Hinnant's algorithm: days since Unix epoch → (year, month, day).
fn days_to_ymd(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(secs_to_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp() {
        // 2026-04-21T10:00:00Z = 1776765600
        assert_eq!(secs_to_iso8601(1_776_765_600), "2026-04-21T10:00:00Z");
    }
}
