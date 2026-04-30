//! Abstraction over NATS KV write operations used at runtime.
//!
//! [`KvOps`] covers only the operations called on the hot path (`put` and
//! `delete`). The initial snapshot load and the background watcher both require
//! the full [`async_nats::jetstream::kv::Store`] and are handled in
//! [`crate::backend::NatsKvVault::new`], which is the only place that needs the
//! real NATS type.
//!
//! [`InMemoryKv`] is available under `#[cfg(test)]` as the mock implementation.

use async_nats::jetstream::kv;
use bytes::Bytes;

use crate::error::NatsKvVaultError;

// ── KvOps trait ───────────────────────────────────────────────────────────────

/// Write operations on a KV bucket used by [`NatsKvVault`](crate::NatsKvVault) at runtime.
///
/// Using `async fn` in a trait requires generics (not `dyn Trait`); the struct
/// is parameterised as `NatsKvVault<K: KvOps>` with `K = kv::Store` as the
/// default so existing call-sites are unchanged.
pub trait KvOps: Send + Sync + 'static {
    fn kv_put(&self, key: &str, value: Bytes)
        -> impl std::future::Future<Output = Result<(), NatsKvVaultError>> + Send;
    /// Idempotent — returns `Ok(())` when the key does not exist.
    fn kv_delete(&self, key: &str)
        -> impl std::future::Future<Output = Result<(), NatsKvVaultError>> + Send;
}

// ── impl KvOps for kv::Store (real NATS) ─────────────────────────────────────

impl KvOps for kv::Store {
    async fn kv_put(&self, key: &str, value: Bytes) -> Result<(), NatsKvVaultError> {
        self.put(key, value)
            .await
            .map(|_| ())
            .map_err(|e| NatsKvVaultError::Nats(e.to_string()))
    }

    async fn kv_delete(&self, key: &str) -> Result<(), NatsKvVaultError> {
        match self.delete(key).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("not found") || msg.contains("NotFound") {
                    Ok(())
                } else {
                    Err(NatsKvVaultError::Nats(msg))
                }
            }
        }
    }
}

// ── InMemoryKv (test mock) ────────────────────────────────────────────────────

/// In-memory [`KvOps`] mock for unit tests.
///
/// Backed by a `DashMap` so tests can inspect written values directly via
/// [`InMemoryKv::get`] after calling vault operations.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct InMemoryKv {
    data: std::sync::Arc<dashmap::DashMap<String, Bytes>>,
}

#[cfg(test)]
impl InMemoryKv {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read back a value written by the vault (still encrypted).
    pub fn get(&self, key: &str) -> Option<Bytes> {
        self.data.get(key).map(|v| v.clone())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }
}

#[cfg(test)]
impl KvOps for InMemoryKv {
    async fn kv_put(&self, key: &str, value: Bytes) -> Result<(), NatsKvVaultError> {
        self.data.insert(key.to_string(), value);
        Ok(())
    }

    async fn kv_delete(&self, key: &str) -> Result<(), NatsKvVaultError> {
        self.data.remove(key);
        Ok(())
    }
}
