//! Deduplication store backed by a NATS JetStream KV bucket.
//!
//! Used in two places:
//! - **telegram-bot bridge** (inbound): dedup by `(session_id, update_id)` to prevent
//!   publishing the same Telegram update twice on webhook/polling retries.
//! - **telegram-agent consumer**: dedup by `event_id` UUID to prevent processing the same
//!   JetStream message twice when it gets redelivered after a crash.

use anyhow::Result;
use async_nats::jetstream::kv::Store;
use tracing::debug;

/// Guards against double-processing the same event.
///
/// All keys live under a `dedup:` namespace inside the provided KV bucket.
/// The bucket TTL (configured at creation time) handles expiry automatically.
#[derive(Clone)]
pub struct DedupStore {
    kv: Store,
}

impl DedupStore {
    pub fn new(kv: Store) -> Self {
        Self { kv }
    }

    /// Returns `true` if `key` has already been seen.
    pub async fn is_seen(&self, key: &str) -> bool {
        let kv_key = format!("dedup:{}", key);
        match self.kv.get(&kv_key).await {
            Ok(Some(_)) => {
                debug!("Duplicate detected: {}", key);
                true
            }
            _ => false,
        }
    }

    /// Mark `key` as seen. The bucket TTL handles expiry.
    pub async fn mark_seen(&self, key: &str) -> Result<()> {
        let kv_key = format!("dedup:{}", key);
        self.kv.put(&kv_key, b"1".to_vec().into()).await?;
        Ok(())
    }

    // ── Convenience wrappers for inbound update dedup ──────────────────────

    /// Returns `true` if this `(session_id, update_id)` pair was already published.
    pub async fn is_update_duplicate(&self, session_id: &str, update_id: i64) -> bool {
        self.is_seen(&format!("upd:{}:{}", session_id, update_id))
            .await
    }

    /// Mark an inbound update as published.
    pub async fn mark_update_seen(&self, session_id: &str, update_id: i64) -> Result<()> {
        self.mark_seen(&format!("upd:{}:{}", session_id, update_id))
            .await
    }
}
