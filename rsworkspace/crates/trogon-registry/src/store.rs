use crate::error::RegistryError;
use async_nats::jetstream::kv;
use bytes::Bytes;
use futures_util::StreamExt as _;
use std::future::Future;

/// Minimal KV operations the registry depends on.
///
/// The real implementation delegates to `async_nats::jetstream::kv::Store`.
/// The mock implementation (behind `test-helpers` / `test`) stores everything
/// in an in-memory `HashMap` so unit tests run without a real NATS server.
pub trait RegistryStore: Send + Sync + Clone + 'static {
    /// Write or overwrite a key. Resets the TTL on the entry.
    fn put(
        &self,
        key: &str,
        value: Bytes,
    ) -> impl Future<Output = Result<(), RegistryError>> + Send;

    /// Read the current value of a key. Returns `None` if the key does not
    /// exist or has expired.
    fn get(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<Bytes>, RegistryError>> + Send;

    /// Mark a key as deleted. Expired keys are removed automatically by the
    /// bucket's `max_age` setting; `delete` is used for explicit de-registration.
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), RegistryError>> + Send;

    /// List all currently live keys in the bucket.
    fn keys(&self) -> impl Future<Output = Result<Vec<String>, RegistryError>> + Send;
}

// ── Production implementation ─────────────────────────────────────────────────

impl RegistryStore for kv::Store {
    async fn put(&self, key: &str, value: Bytes) -> Result<(), RegistryError> {
        kv::Store::put(self, key, value)
            .await
            .map(|_| ())
            .map_err(|e| RegistryError::Put(e.to_string()))
    }

    async fn get(&self, key: &str) -> Result<Option<Bytes>, RegistryError> {
        kv::Store::get(self, key)
            .await
            .map_err(|e| RegistryError::Get(e.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), RegistryError> {
        kv::Store::delete(self, key)
            .await
            .map_err(|e| RegistryError::Delete(e.to_string()))
    }

    async fn keys(&self) -> Result<Vec<String>, RegistryError> {
        let mut stream = kv::Store::keys(self)
            .await
            .map_err(|e| RegistryError::List(e.to_string()))?;
        let mut keys = Vec::new();
        while let Some(key) = stream.next().await {
            keys.push(key.map_err(|e| RegistryError::List(e.to_string()))?);
        }
        Ok(keys)
    }
}

// ── Test / mock implementation ────────────────────────────────────────────────

#[cfg(any(test, feature = "test-helpers"))]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// In-memory registry store for unit tests.
    ///
    /// Thread-safe via `Arc<Mutex<…>>`. Cloning shares the same underlying map
    /// so that a cloned handle (e.g. passed into `Registry::new`) sees all
    /// mutations made through the original.
    #[derive(Clone, Default)]
    pub struct MockRegistryStore {
        data: Arc<Mutex<HashMap<String, Bytes>>>,
    }

    impl MockRegistryStore {
        pub fn new() -> Self {
            Self::default()
        }

        /// Snapshot of every key currently in the store.
        pub fn snapshot(&self) -> HashMap<String, Bytes> {
            self.data.lock().unwrap().clone()
        }
    }

    impl RegistryStore for MockRegistryStore {
        async fn put(&self, key: &str, value: Bytes) -> Result<(), RegistryError> {
            self.data.lock().unwrap().insert(key.to_string(), value);
            Ok(())
        }

        async fn get(&self, key: &str) -> Result<Option<Bytes>, RegistryError> {
            Ok(self.data.lock().unwrap().get(key).cloned())
        }

        async fn delete(&self, key: &str) -> Result<(), RegistryError> {
            self.data.lock().unwrap().remove(key);
            Ok(())
        }

        async fn keys(&self) -> Result<Vec<String>, RegistryError> {
            Ok(self.data.lock().unwrap().keys().cloned().collect())
        }
    }
}
