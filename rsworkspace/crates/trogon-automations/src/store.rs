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

use std::future::Future;
use std::pin::Pin;

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
        match self
            .kv
            .get(&key)
            .await
            .map_err(|e| StoreError(e.to_string()))?
        {
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
        self.kv
            .delete(&key)
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    /// Return all automations belonging to `tenant_id`.
    pub async fn list(&self, tenant_id: &str) -> Result<Vec<Automation>, StoreError> {
        let prefix = format!("{tenant_id}.");
        let mut keys = self
            .kv
            .keys()
            .await
            .map_err(|e| StoreError(e.to_string()))?;
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
    ) -> Result<futures_util::stream::BoxStream<'static, (String, Option<Automation>)>, StoreError>
    {
        let watcher = self
            .kv
            .watch_with_history(">")
            .await
            .map_err(|e| StoreError(e.to_string()))?;

        Ok(Box::pin(watcher.filter_map(|entry| async move {
            let entry = entry.ok()?;
            let key = entry.key.clone();
            match entry.operation {
                kv::Operation::Delete | kv::Operation::Purge => Some((key, None)),
                kv::Operation::Put => {
                    let a = serde_json::from_slice::<Automation>(&entry.value).ok()?;
                    Some((key, Some(a)))
                }
            }
        })))
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

// ── Repository trait ──────────────────────────────────────────────────────────

/// Abstraction over an automation configuration store.
pub trait AutomationRepository: Clone + Send + Sync + 'static {
    fn put<'a>(
        &'a self,
        automation: &'a Automation,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>>;

    fn get<'a>(
        &'a self,
        tenant_id: &'a str,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Automation>, StoreError>> + Send + 'a>>;

    fn delete<'a>(
        &'a self,
        tenant_id: &'a str,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>>;

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Automation>, StoreError>> + Send + 'a>>;

    fn matching<'a>(
        &'a self,
        tenant_id: &'a str,
        nats_subject: &'a str,
        payload: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Automation>, StoreError>> + Send + 'a>>;
}

impl AutomationRepository for AutomationStore {
    fn put<'a>(
        &'a self,
        automation: &'a Automation,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
        Box::pin(async move { self.put(automation).await })
    }

    fn get<'a>(
        &'a self,
        tenant_id: &'a str,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Automation>, StoreError>> + Send + 'a>> {
        Box::pin(async move { self.get(tenant_id, id).await })
    }

    fn delete<'a>(
        &'a self,
        tenant_id: &'a str,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
        Box::pin(async move { self.delete(tenant_id, id).await })
    }

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Automation>, StoreError>> + Send + 'a>> {
        Box::pin(async move { self.list(tenant_id).await })
    }

    fn matching<'a>(
        &'a self,
        tenant_id: &'a str,
        nats_subject: &'a str,
        payload: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Automation>, StoreError>> + Send + 'a>> {
        Box::pin(async move { self.matching(tenant_id, nats_subject, payload).await })
    }
}

// ── In-memory mock (test-only) ────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// HashMap-backed in-memory automation store for unit tests.
    #[derive(Clone, Default)]
    pub struct MockAutomationStore {
        data: Arc<Mutex<HashMap<String, Automation>>>,
    }

    impl MockAutomationStore {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn insert(&self, automation: Automation) {
            let key = format!("{}.{}", automation.tenant_id, automation.id);
            self.data.lock().unwrap().insert(key, automation);
        }

        pub fn snapshot(&self) -> HashMap<String, Automation> {
            self.data.lock().unwrap().clone()
        }
    }

    impl AutomationRepository for MockAutomationStore {
        fn put<'a>(
            &'a self,
            automation: &'a Automation,
        ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let automation = automation.clone();
            Box::pin(async move {
                let key = format!("{}.{}", automation.tenant_id, automation.id);
                data.lock().unwrap().insert(key, automation);
                Ok(())
            })
        }

        fn get<'a>(
            &'a self,
            tenant_id: &'a str,
            id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Automation>, StoreError>> + Send + 'a>>
        {
            let data = Arc::clone(&self.data);
            let key = format!("{tenant_id}.{id}");
            Box::pin(async move { Ok(data.lock().unwrap().get(&key).cloned()) })
        }

        fn delete<'a>(
            &'a self,
            tenant_id: &'a str,
            id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let key = format!("{tenant_id}.{id}");
            Box::pin(async move {
                data.lock().unwrap().remove(&key);
                Ok(())
            })
        }

        fn list<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Automation>, StoreError>> + Send + 'a>>
        {
            let data = Arc::clone(&self.data);
            let prefix = format!("{tenant_id}.");
            Box::pin(async move {
                let guard = data.lock().unwrap();
                Ok(guard
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, v)| v.clone())
                    .collect())
            })
        }

        fn matching<'a>(
            &'a self,
            tenant_id: &'a str,
            nats_subject: &'a str,
            payload: &'a serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Automation>, StoreError>> + Send + 'a>>
        {
            let data = Arc::clone(&self.data);
            let prefix = format!("{tenant_id}.");
            let nats_subject = nats_subject.to_string();
            let payload = payload.clone();
            Box::pin(async move {
                let guard = data.lock().unwrap();
                Ok(guard
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, v)| v.clone())
                    .filter(|a| {
                        a.enabled && crate::trigger::matches(&a.trigger, &nats_subject, &payload)
                    })
                    .collect())
            })
        }
    }
}

#[cfg(test)]
pub mod error_mock {
    use super::*;

    /// Automation store that always returns errors — used to test 500 paths.
    #[derive(Clone, Default)]
    pub struct ErrorAutomationStore;

    impl ErrorAutomationStore {
        pub fn new() -> Self {
            Self
        }
    }

    impl AutomationRepository for ErrorAutomationStore {
        fn put<'a>(
            &'a self,
            _automation: &'a Automation,
        ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
            Box::pin(async move { Err(StoreError("injected put error".into())) })
        }

        fn get<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Automation>, StoreError>> + Send + 'a>>
        {
            Box::pin(async move { Err(StoreError("injected get error".into())) })
        }

        fn delete<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
            Box::pin(async move { Err(StoreError("injected delete error".into())) })
        }

        fn list<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Automation>, StoreError>> + Send + 'a>> {
            Box::pin(async move { Err(StoreError("injected list error".into())) })
        }

        fn matching<'a>(
            &'a self,
            _tenant_id: &'a str,
            _nats_subject: &'a str,
            _payload: &'a serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Automation>, StoreError>> + Send + 'a>> {
            Box::pin(async move { Err(StoreError("injected matching error".into())) })
        }
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
    fn store_error_source_is_none() {
        let e = StoreError("some error".to_string());
        assert!(
            std::error::Error::source(&e).is_none(),
            "StoreError::source must be None — no underlying cause"
        );
    }

    #[test]
    fn kv_key_format() {
        assert_eq!(kv_key("acme", "uuid-123"), "acme.uuid-123");
        assert_eq!(kv_key("org-1", "auto-abc"), "org-1.auto-abc");
    }
}
