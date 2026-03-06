//! Persistent storage for automations — backed by NATS JetStream Key-Value.
//!
//! # Bucket
//! All automations are stored in a single KV bucket called `AUTOMATIONS`.
//! Each entry is a JSON-serialised [`Automation`] keyed by its UUID `id`.
//!
//! # Live reload
//! [`AutomationStore::watch`] returns a stream that emits changes as they
//! happen — the runner uses this to keep its in-memory cache up to date
//! without restarting.

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt as _;
use tracing::warn;

use crate::automation::Automation;

/// NATS KV bucket name.
pub const BUCKET: &str = "AUTOMATIONS";

/// An error from the automation store.
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// NATS KV-backed store for [`Automation`] configs.
#[derive(Clone)]
pub struct AutomationStore {
    kv: kv::Store,
}

impl AutomationStore {
    /// Open (or create) the `AUTOMATIONS` KV bucket.
    pub async fn open(js: &jetstream::Context) -> Result<Self, StoreError> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: BUCKET.to_string(),
                history: 5,
                ..Default::default()
            })
            .await
            .map_err(|e| StoreError(e.to_string()))?;

        Ok(Self { kv })
    }

    /// Persist an automation (create or overwrite).
    pub async fn put(&self, automation: &Automation) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(automation).map_err(|e| StoreError(e.to_string()))?;
        self.kv
            .put(&automation.id, Bytes::from(bytes))
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    /// Fetch a single automation by ID.  Returns `None` if not found.
    pub async fn get(&self, id: &str) -> Result<Option<Automation>, StoreError> {
        match self.kv.get(id).await.map_err(|e| StoreError(e.to_string()))? {
            None => Ok(None),
            Some(bytes) => {
                let a = serde_json::from_slice::<Automation>(&bytes)
                    .map_err(|e| StoreError(e.to_string()))?;
                Ok(Some(a))
            }
        }
    }

    /// Delete an automation by ID.
    pub async fn delete(&self, id: &str) -> Result<(), StoreError> {
        self.kv.delete(id).await.map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    /// Return all currently stored automations.
    pub async fn list(&self) -> Result<Vec<Automation>, StoreError> {
        let mut keys = self.kv.keys().await.map_err(|e| StoreError(e.to_string()))?;
        let mut result = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| StoreError(e.to_string()))?;
            match self.get(&key).await {
                Ok(Some(a)) => result.push(a),
                Ok(None) => {}
                Err(e) => warn!(key, error = %e, "Skipping unreadable automation"),
            }
        }
        Ok(result)
    }

    /// Return a stream of live KV changes.
    ///
    /// Delivers the current snapshot first (one item per existing key), then
    /// continues with incremental updates.  Each item is `(id, Some(auto))`
    /// for creates/updates or `(id, None)` for deletes.
    pub async fn watch(
        &self,
    ) -> Result<impl futures_util::Stream<Item = (String, Option<Automation>)>, StoreError> {
        let watcher = self
            .kv
            .watch_with_history(">")
            .await
            .map_err(|e| StoreError(e.to_string()))?;

        Ok(watcher.filter_map(|entry| async move {
            let entry = entry.ok()?;
            let key = entry.key.clone();
            match entry.operation {
                kv::Operation::Delete | kv::Operation::Purge => Some((key, None)),
                kv::Operation::Put => {
                    let a = serde_json::from_slice::<Automation>(&entry.value).ok()?;
                    Some((key, Some(a)))
                }
            }
        }))
    }

    /// Return all enabled automations whose trigger matches `nats_subject`
    /// and the given event `payload`.
    pub async fn matching(
        &self,
        nats_subject: &str,
        payload: &serde_json::Value,
    ) -> Result<Vec<Automation>, StoreError> {
        let all = self.list().await?;
        Ok(all
            .into_iter()
            .filter(|a| a.enabled && crate::trigger::matches(&a.trigger, nats_subject, payload))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit-level: StoreError display forwards the inner message.
    #[test]
    fn store_error_display() {
        let e = StoreError("bucket gone".to_string());
        assert!(e.to_string().contains("bucket gone"));
    }
}
