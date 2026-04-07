//! NATS KV store for incident state.
//!
//! Maintains the current state of every incident received via webhook.
//! Each KV key is the incident ID; the value is the full incident JSON
//! from the webhook payload.  This lets the agent (or any consumer) read
//! the latest known state of an incident without hitting the incident.io API.
//!
//! The underlying JetStream KV bucket (`INCIDENTS`) keeps 10 revisions per key,
//! so recent history is preserved for auditing.

use std::future::Future;
use std::pin::Pin;

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt;

pub const BUCKET: &str = "INCIDENTS";

/// Error from the incident store.
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// Abstraction over an incident state store.
///
/// Implementing this trait allows replacing the real NATS KV backend with a
/// lightweight in-memory fake in unit tests.
pub trait IncidentRepository: Clone + Send + Sync + 'static {
    fn upsert<'a>(
        &'a self,
        id: &'a str,
        json: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>>;

    fn get<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<u8>>, StoreError>> + Send + 'a>>;

    fn list<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<u8>>, StoreError>> + Send + 'a>>;
}

/// NATS KV-backed store for incident state.
#[derive(Clone)]
pub struct IncidentStore {
    kv: kv::Store,
}

impl IncidentStore {
    /// Open (or create) the `INCIDENTS` KV bucket.
    pub async fn open(js: &jetstream::Context) -> Result<Self, StoreError> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: BUCKET.to_string(),
                history: 10,
                ..Default::default()
            })
            .await
            .map_err(|e| StoreError(e.to_string()))?;

        Ok(Self { kv })
    }

    /// Upsert the current state of an incident (keyed by incident ID).
    pub async fn upsert(&self, incident_id: &str, incident_json: &[u8]) -> Result<(), StoreError> {
        self.kv
            .put(incident_id, Bytes::copy_from_slice(incident_json))
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    /// Retrieve the stored state of an incident by ID.
    ///
    /// Returns `None` when the incident has not been seen yet.
    pub async fn get(&self, incident_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        match self
            .kv
            .get(incident_id)
            .await
            .map_err(|e| StoreError(e.to_string()))?
        {
            None => Ok(None),
            Some(bytes) => Ok(Some(bytes.to_vec())),
        }
    }

    /// List all stored incidents.
    ///
    /// Iterates over all keys in the KV bucket and returns the latest value for
    /// each incident.  Keys with no value (deleted entries) are skipped.
    pub async fn list(&self) -> Result<Vec<Vec<u8>>, StoreError> {
        let mut keys = self
            .kv
            .keys()
            .await
            .map_err(|e| StoreError(e.to_string()))?;

        let mut results = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| StoreError(e.to_string()))?;
            if let Some(bytes) = self
                .kv
                .get(&key)
                .await
                .map_err(|e| StoreError(e.to_string()))?
            {
                results.push(bytes.to_vec());
            }
        }
        Ok(results)
    }
}

impl IncidentRepository for IncidentStore {
    fn upsert<'a>(
        &'a self,
        id: &'a str,
        json: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
        Box::pin(async move { self.upsert(id, json).await })
    }

    fn get<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<u8>>, StoreError>> + Send + 'a>> {
        Box::pin(async move { self.get(id).await })
    }

    fn list<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<u8>>, StoreError>> + Send + 'a>> {
        Box::pin(async move { self.list().await })
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct MockIncidentStore {
        data: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    impl MockIncidentStore {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn snapshot(&self) -> HashMap<String, Vec<u8>> {
            self.data.lock().unwrap().clone()
        }
    }

    impl IncidentRepository for MockIncidentStore {
        fn upsert<'a>(
            &'a self,
            id: &'a str,
            json: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let id = id.to_string();
            let json = json.to_vec();
            Box::pin(async move {
                data.lock().unwrap().insert(id, json);
                Ok(())
            })
        }

        fn get<'a>(
            &'a self,
            id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<u8>>, StoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            let id = id.to_string();
            Box::pin(async move { Ok(data.lock().unwrap().get(&id).cloned()) })
        }

        fn list<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<u8>>, StoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.data);
            Box::pin(async move { Ok(data.lock().unwrap().values().cloned().collect()) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

    async fn open_store() -> (impl Drop, IncidentStore) {
        let container = Nats::default()
            .with_cmd(["--jetstream"])
            .start()
            .await
            .expect("Docker must be running for store tests");
        let port = container.get_host_port_ipv4(4222).await.unwrap();
        let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
            .await
            .unwrap();
        let js = async_nats::jetstream::new(nats);
        let store = IncidentStore::open(&js).await.unwrap();
        (container, store)
    }

    #[tokio::test]
    async fn open_creates_kv_bucket() {
        let (_c, store) = open_store().await;
        store.upsert("probe", b"{}").await.unwrap();
        let v = store.get("probe").await.unwrap();
        assert!(v.is_some());
    }

    #[tokio::test]
    async fn upsert_and_get_roundtrip() {
        let (_c, store) = open_store().await;
        let data = br#"{"id":"inc-1","name":"disk full"}"#;
        store.upsert("inc-1", data).await.unwrap();
        let got = store.get("inc-1").await.unwrap().expect("should be Some");
        assert_eq!(got, data);
    }

    #[tokio::test]
    async fn upsert_overwrites_existing_entry() {
        let (_c, store) = open_store().await;
        store.upsert("inc-2", b"original").await.unwrap();
        store.upsert("inc-2", b"updated").await.unwrap();
        let got = store.get("inc-2").await.unwrap().unwrap();
        assert_eq!(got, b"updated");
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_key() {
        let (_c, store) = open_store().await;
        let result = store.get("does-not-exist").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_empty_bucket_returns_empty_vec() {
        let (_c, store) = open_store().await;
        let result = store.list().await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        // Opening the same KV bucket twice must succeed — create_or_update is safe to call
        // multiple times on the same JetStream context.
        let container = Nats::default()
            .with_cmd(["--jetstream"])
            .start()
            .await
            .expect("Docker must be running for store tests");
        let port = container.get_host_port_ipv4(4222).await.unwrap();
        let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
            .await
            .unwrap();
        let js = async_nats::jetstream::new(nats);

        let store1 = IncidentStore::open(&js).await.unwrap();
        let store2 = IncidentStore::open(&js).await.unwrap();

        // Both stores must work independently.
        store1.upsert("inc-a", b"a").await.unwrap();
        let got = store2.get("inc-a").await.unwrap();
        assert_eq!(got.as_deref(), Some(b"a".as_slice()));
    }

    #[tokio::test]
    async fn list_returns_all_incidents() {
        let (_c, store) = open_store().await;
        store.upsert("inc-a", b"payload-a").await.unwrap();
        store.upsert("inc-b", b"payload-b").await.unwrap();
        store.upsert("inc-c", b"payload-c").await.unwrap();

        let mut results = store.list().await.unwrap();
        results.sort();
        assert_eq!(results.len(), 3);
        assert!(results.contains(&b"payload-a".to_vec()));
        assert!(results.contains(&b"payload-b".to_vec()));
        assert!(results.contains(&b"payload-c".to_vec()));
    }
}
