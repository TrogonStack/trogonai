//! A [`RegistryStore`] that re-provisions the KV bucket when an operation fails.
//!
//! MED-36: the `AGENT_REGISTRY` bucket uses in-memory storage, so a NATS server
//! restart destroys it. A `kv::Store` handle captured once at startup then becomes
//! stale and every `get`/`keys`/`put` fails until the CLI is restarted. This
//! wrapper holds the JetStream context, and on any operation error it re-provisions
//! the bucket once and retries — so the registry recovers on its own (agents
//! re-register via their heartbeats).

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use std::sync::{Arc, RwLock};

use crate::provision::provision;
use crate::store::{KeysCollectError, RegistryStore};

/// Registry store that transparently re-provisions the bucket on failure.
#[derive(Clone)]
pub struct ReprovisioningStore {
    js: jetstream::Context,
    store: Arc<RwLock<kv::Store>>,
}

impl ReprovisioningStore {
    /// Provision the bucket and wrap the resulting store.
    pub async fn new(js: jetstream::Context) -> Result<Self, crate::RegistryError> {
        let store = provision(&js).await?;
        Ok(Self {
            js,
            store: Arc::new(RwLock::new(store)),
        })
    }

    /// Re-create the bucket handle (best-effort) and swap it in.
    async fn reprovision(&self) {
        // Note: the lock guard is never held across an await — provision runs
        // first, then the write lock is taken and released synchronously.
        if let Ok(fresh) = provision(&self.js).await {
            if let Ok(mut guard) = self.store.write() {
                *guard = fresh;
            }
        }
    }

    fn current(&self) -> kv::Store {
        self.store
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }
}

impl RegistryStore for ReprovisioningStore {
    type PutError = kv::PutError;
    type GetError = kv::EntryError;
    type DeleteError = kv::DeleteError;
    type KeysError = KeysCollectError;

    async fn put(&self, key: &str, value: Bytes) -> Result<u64, Self::PutError> {
        match RegistryStore::put(&self.current(), key, value.clone()).await {
            Ok(rev) => Ok(rev),
            Err(_) => {
                self.reprovision().await;
                RegistryStore::put(&self.current(), key, value).await
            }
        }
    }

    async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
        match RegistryStore::get(&self.current(), key).await {
            Ok(v) => Ok(v),
            Err(_) => {
                self.reprovision().await;
                RegistryStore::get(&self.current(), key).await
            }
        }
    }

    async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
        match RegistryStore::delete(&self.current(), key).await {
            Ok(()) => Ok(()),
            Err(_) => {
                self.reprovision().await;
                RegistryStore::delete(&self.current(), key).await
            }
        }
    }

    async fn keys(&self) -> Result<Vec<String>, Self::KeysError> {
        match RegistryStore::keys(&self.current()).await {
            Ok(keys) => Ok(keys),
            Err(_) => {
                self.reprovision().await;
                RegistryStore::keys(&self.current()).await
            }
        }
    }
}
