//! Persistent storage for automations — backed by NATS JetStream Key-Value.
//!
//! # Bucket
//! All automations are stored in a single KV bucket called `AUTOMATIONS`.
//!
//! # Tenant isolation
//! Each KV key is prefixed with the tenant ID: `{tenant_id}.{automation_id}`.
//! This keeps all tenants in one bucket while preventing cross-tenant reads.
//!
//! # Live reload
//! [`AutomationStore::watch`] returns a stream of all changes across all
//! tenants — the runner filters by its own `tenant_id`.

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt as _;
use tracing::warn;

use crate::automation::Automation;

/// NATS KV bucket name (shared across all tenants).
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

/// Build the KV key for a tenant + automation id.
fn kv_key(tenant_id: &str, id: &str) -> String {
    format!("{tenant_id}.{id}")
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
    ///
    /// The KV key is derived from `automation.tenant_id` and `automation.id`.
    pub async fn put(&self, automation: &Automation) -> Result<(), StoreError> {
        let key = kv_key(&automation.tenant_id, &automation.id);
        let bytes = serde_json::to_vec(automation).map_err(|e| StoreError(e.to_string()))?;
        self.kv
            .put(&key, Bytes::from(bytes))
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    /// Fetch a single automation by tenant + id.  Returns `None` if not found.
    pub async fn get(&self, tenant_id: &str, id: &str) -> Result<Option<Automation>, StoreError> {
        let key = kv_key(tenant_id, id);
        match self.kv.get(&key).await.map_err(|e| StoreError(e.to_string()))? {
            None => Ok(None),
            Some(bytes) => {
                let a = serde_json::from_slice::<Automation>(&bytes)
                    .map_err(|e| StoreError(e.to_string()))?;
                Ok(Some(a))
            }
        }
    }

    /// Delete an automation by tenant + id.
    pub async fn delete(&self, tenant_id: &str, id: &str) -> Result<(), StoreError> {
        let key = kv_key(tenant_id, id);
        self.kv.delete(&key).await.map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    /// Return all automations belonging to `tenant_id`.
    pub async fn list(&self, tenant_id: &str) -> Result<Vec<Automation>, StoreError> {
        let prefix = format!("{tenant_id}.");
        let mut keys = self.kv.keys().await.map_err(|e| StoreError(e.to_string()))?;
        let mut result = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| StoreError(e.to_string()))?;
            if !key.starts_with(&prefix) {
                continue;
            }
            let id = &key[prefix.len()..];
            match self.get(tenant_id, id).await {
                Ok(Some(a)) => result.push(a),
                Ok(None) => {}
                Err(e) => warn!(key, error = %e, "Skipping unreadable automation"),
            }
        }
        Ok(result)
    }

    /// Return a stream of live KV changes across **all** tenants.
    ///
    /// Delivers the current snapshot first, then incremental updates.
    /// Each item is `(kv_key, Some(auto))` for puts or `(kv_key, None)` for deletes.
    /// The runner filters items by `automation.tenant_id`.
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

    /// Return all enabled automations for `tenant_id` whose trigger matches
    /// `nats_subject` and the given event `payload`.
    pub async fn matching(
        &self,
        tenant_id: &str,
        nats_subject: &str,
        payload: &serde_json::Value,
    ) -> Result<Vec<Automation>, StoreError> {
        let all = self.list(tenant_id).await?;
        Ok(all
            .into_iter()
            .filter(|a| a.enabled && crate::trigger::matches(&a.trigger, nats_subject, payload))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_error_display() {
        let e = StoreError("bucket gone".to_string());
        assert!(e.to_string().contains("bucket gone"));
    }

    #[test]
    fn kv_key_format() {
        assert_eq!(kv_key("acme", "uuid-123"), "acme.uuid-123");
        assert_eq!(kv_key("org-1", "auto-abc"), "org-1.auto-abc");
    }
}
