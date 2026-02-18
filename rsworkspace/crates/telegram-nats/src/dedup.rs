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
        let kv_key = format!("dedup.{}", key.replace(':', "."));
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
        let kv_key = format!("dedup.{}", key.replace(':', "."));
        self.kv.put(&kv_key, b"1".to_vec().into()).await?;
        Ok(())
    }

    // ── Convenience wrappers for inbound update dedup ──────────────────────

    /// Returns `true` if this `(session_id, update_id)` pair was already published.
    pub async fn is_update_duplicate(&self, session_id: &str, update_id: i64) -> bool {
        self.is_seen(&format!("upd.{}.{}", session_id, update_id))
            .await
    }

    /// Mark an inbound update as published.
    pub async fn mark_update_seen(&self, session_id: &str, update_id: i64) -> Result<()> {
        self.mark_seen(&format!("upd.{}.{}", session_id, update_id))
            .await
    }
}

// ── Issue 2: DedupStore tests ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const NATS_URL: &str = "nats://localhost:14222";

    async fn try_kv() -> Option<async_nats::jetstream::kv::Store> {
        let client = async_nats::connect(NATS_URL).await.ok()?;
        let js = async_nats::jetstream::new(client);
        let bucket = format!("test-dedup-{}", uuid::Uuid::new_v4().simple());
        js.create_key_value(async_nats::jetstream::kv::Config {
            bucket,
            ..Default::default()
        })
        .await
        .ok()
    }

    #[tokio::test]
    async fn test_unseen_key_returns_false() {
        // is_seen must return false for a key that was never marked
        let Some(kv) = try_kv().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let store = DedupStore::new(kv);
        assert!(
            !store.is_seen("never-seen-key").await,
            "unseen key must return false"
        );
    }

    #[tokio::test]
    async fn test_mark_then_is_seen_returns_true() {
        // mark_seen → is_seen must return true (core dedup contract)
        let Some(kv) = try_kv().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let store = DedupStore::new(kv);
        let key = format!("evt:{}", uuid::Uuid::new_v4());
        store.mark_seen(&key).await.unwrap();
        assert!(store.is_seen(&key).await, "marked key must be seen");
    }

    #[tokio::test]
    async fn test_different_keys_do_not_interfere() {
        // Marking key-A must not affect key-B (namespace isolation)
        let Some(kv) = try_kv().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let store = DedupStore::new(kv);
        store.mark_seen("key-A").await.unwrap();
        assert!(store.is_seen("key-A").await);
        assert!(!store.is_seen("key-B").await, "key-B must still be unseen");
    }

    #[tokio::test]
    async fn test_update_dedup_wrappers_same_session() {
        // Convenience wrappers: (session_id, update_id) dedup for inbound bridge
        let Some(kv) = try_kv().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let store = DedupStore::new(kv);
        let session = "private:99999";
        let update_id: i64 = 42;

        assert!(!store.is_update_duplicate(session, update_id).await);
        store.mark_update_seen(session, update_id).await.unwrap();
        assert!(store.is_update_duplicate(session, update_id).await);
    }

    #[tokio::test]
    async fn test_update_dedup_different_sessions_isolated() {
        // Same update_id on two different sessions must be independent
        let Some(kv) = try_kv().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let store = DedupStore::new(kv);
        let update_id: i64 = 100;

        store.mark_update_seen("session-A", update_id).await.unwrap();
        assert!(store.is_update_duplicate("session-A", update_id).await);
        assert!(
            !store.is_update_duplicate("session-B", update_id).await,
            "different session must not be marked"
        );
    }
}
