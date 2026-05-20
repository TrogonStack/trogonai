use async_nats::jetstream::kv;
use bytes::Bytes;
use futures_util::StreamExt as _;
use std::future::Future;

/// Minimal KV operations the registry depends on.
///
/// The real implementation delegates to `async_nats::jetstream::kv::Store`.
/// The mock implementation (behind `test-support` / `test`) stores everything
/// in an in-memory `HashMap` so unit tests run without a real NATS server.
pub trait RegistryStore: Send + Sync + Clone + 'static {
    type PutError: std::error::Error + Send + Sync + 'static;
    type GetError: std::error::Error + Send + Sync + 'static;
    type DeleteError: std::error::Error + Send + Sync + 'static;
    type KeysError: std::error::Error + Send + Sync + 'static;

    /// Write or overwrite a key. Resets the TTL on the entry.
    fn put(
        &self,
        key: &str,
        value: Bytes,
    ) -> impl Future<Output = Result<u64, Self::PutError>> + Send;

    /// Read the current value of a key. Returns `None` if the key does not
    /// exist or has expired.
    fn get(&self, key: &str) -> impl Future<Output = Result<Option<Bytes>, Self::GetError>> + Send;

    /// Mark a key as deleted. Expired keys are removed automatically by the
    /// bucket's `max_age` setting; `delete` is used for explicit de-registration.
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Self::DeleteError>> + Send;

    /// List all currently live keys in the bucket.
    fn keys(&self) -> impl Future<Output = Result<Vec<String>, Self::KeysError>> + Send;
}

// ── Production implementation ─────────────────────────────────────────────────

/// Typed error for the two-phase keys collection (open stream + iterate items).
#[derive(Debug)]
pub enum KeysCollectError {
    /// Failed to open the keys stream.
    Open(kv::HistoryError),
    /// Failed while iterating an item from the keys stream.
    Item(kv::WatcherError),
}

impl std::fmt::Display for KeysCollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeysCollectError::Open(e) => write!(f, "failed to open keys stream: {e}"),
            KeysCollectError::Item(e) => write!(f, "failed to read key from stream: {e}"),
        }
    }
}

impl std::error::Error for KeysCollectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KeysCollectError::Open(e) => Some(e),
            KeysCollectError::Item(e) => Some(e),
        }
    }
}

impl RegistryStore for kv::Store {
    type PutError = kv::PutError;
    type GetError = kv::EntryError;
    type DeleteError = kv::DeleteError;
    type KeysError = KeysCollectError;

    async fn put(&self, key: &str, value: Bytes) -> Result<u64, Self::PutError> {
        kv::Store::put(self, key, value).await
    }

    async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
        kv::Store::get(self, key).await
    }

    async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
        kv::Store::delete(self, key).await
    }

    async fn keys(&self) -> Result<Vec<String>, Self::KeysError> {
        let mut stream = kv::Store::keys(self)
            .await
            .map_err(KeysCollectError::Open)?;
        let mut keys = Vec::new();
        while let Some(key) = stream.next().await {
            keys.push(key.map_err(KeysCollectError::Item)?);
        }
        Ok(keys)
    }
}

// ── Test / mock implementation ────────────────────────────────────────────────

#[cfg(any(test, feature = "test-support"))]
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
        type PutError = std::convert::Infallible;
        type GetError = std::convert::Infallible;
        type DeleteError = std::convert::Infallible;
        type KeysError = std::convert::Infallible;

        async fn put(&self, key: &str, value: Bytes) -> Result<u64, Self::PutError> {
            self.data.lock().unwrap().insert(key.to_string(), value);
            Ok(0)
        }

        async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
            Ok(self.data.lock().unwrap().get(key).cloned())
        }

        async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
            self.data.lock().unwrap().remove(key);
            Ok(())
        }

        async fn keys(&self) -> Result<Vec<String>, Self::KeysError> {
            Ok(self.data.lock().unwrap().keys().cloned().collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use super::RegistryStore as _;
    use super::mock::MockRegistryStore;

    #[tokio::test]
    async fn put_then_get_returns_value() {
        let store = MockRegistryStore::new();
        store.put("key-a", Bytes::from("value-a")).await.unwrap();
        let got = store.get("key-a").await.unwrap();
        assert_eq!(got, Some(Bytes::from("value-a")));
    }

    #[tokio::test]
    async fn get_missing_key_returns_none() {
        let store = MockRegistryStore::new();
        let got = store.get("no-such-key").await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn delete_removes_key() {
        let store = MockRegistryStore::new();
        store.put("del-key", Bytes::from("v")).await.unwrap();
        store.delete("del-key").await.unwrap();
        assert!(store.get("del-key").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_key_is_noop() {
        let store = MockRegistryStore::new();
        store.delete("ghost").await.unwrap();
    }

    #[tokio::test]
    async fn keys_returns_all_live_keys() {
        let store = MockRegistryStore::new();
        store.put("k1", Bytes::from("v1")).await.unwrap();
        store.put("k2", Bytes::from("v2")).await.unwrap();
        store.put("k3", Bytes::from("v3")).await.unwrap();
        let mut keys = store.keys().await.unwrap();
        keys.sort();
        assert_eq!(keys, vec!["k1", "k2", "k3"]);
    }

    #[tokio::test]
    async fn keys_excludes_deleted_keys() {
        let store = MockRegistryStore::new();
        store.put("live", Bytes::from("v")).await.unwrap();
        store.put("gone", Bytes::from("v")).await.unwrap();
        store.delete("gone").await.unwrap();
        let keys = store.keys().await.unwrap();
        assert_eq!(keys, vec!["live"]);
    }

    #[tokio::test]
    async fn snapshot_reflects_current_state() {
        let store = MockRegistryStore::new();
        store.put("snap-k", Bytes::from("snap-v")).await.unwrap();
        let snap = store.snapshot();
        assert_eq!(snap.get("snap-k").cloned(), Some(Bytes::from("snap-v")));
        assert_eq!(snap.len(), 1);
    }

    #[tokio::test]
    async fn clone_shares_underlying_storage() {
        let store = MockRegistryStore::new();
        let clone = store.clone();
        store.put("shared", Bytes::from("data")).await.unwrap();
        let got = clone.get("shared").await.unwrap();
        assert_eq!(got, Some(Bytes::from("data")));
    }

    #[tokio::test]
    async fn put_overwrites_existing_value() {
        let store = MockRegistryStore::new();
        store.put("key", Bytes::from("v1")).await.unwrap();
        store.put("key", Bytes::from("v2")).await.unwrap();
        let got = store.get("key").await.unwrap();
        assert_eq!(got, Some(Bytes::from("v2")));
    }
}
