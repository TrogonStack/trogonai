//! Deduplication store for inbound events

use anyhow::Result;
use async_nats::jetstream::kv::Store;
use tracing::debug;
use uuid::Uuid;

/// Guards against double-processing the same event.
///
/// Uses the Telegram update_id as the stable dedup key for inbound events.
/// The KV bucket has a 24h TTL so entries expire automatically.
#[derive(Clone)]
pub struct DedupStore {
    kv: Store,
}

impl DedupStore {
    pub fn new(kv: Store) -> Self {
        Self { kv }
    }

    /// Returns `true` if this event_id (UUID) has already been processed.
    pub async fn is_duplicate(&self, event_id: &Uuid) -> bool {
        let key = format!("dedup:{}", event_id);
        match self.kv.get(&key).await {
            Ok(Some(_)) => {
                debug!("Duplicate event detected: {}", event_id);
                true
            }
            _ => false,
        }
    }

    /// Mark an event_id as seen. The KV bucket TTL (24h) handles expiry.
    pub async fn mark_seen(&self, event_id: &Uuid) -> Result<()> {
        let key = format!("dedup:{}", event_id);
        self.kv.put(&key, b"1".to_vec().into()).await?;
        Ok(())
    }

    /// Returns `true` if this update_id has already been processed.
    pub async fn is_update_duplicate(&self, session_id: &str, update_id: i64) -> bool {
        let key = format!("dedup:upd:{}:{}", session_id, update_id);
        match self.kv.get(&key).await {
            Ok(Some(_)) => {
                debug!("Duplicate update detected: {}:{}", session_id, update_id);
                true
            }
            _ => false,
        }
    }

    /// Mark an update as seen.
    pub async fn mark_update_seen(&self, session_id: &str, update_id: i64) -> Result<()> {
        let key = format!("dedup:upd:{}:{}", session_id, update_id);
        self.kv.put(&key, b"1".to_vec().into()).await?;
        Ok(())
    }
}
