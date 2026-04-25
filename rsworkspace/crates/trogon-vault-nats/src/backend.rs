use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream::kv;
use dashmap::DashMap;
use futures_util::StreamExt as _;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use trogon_vault::{ApiKeyToken, VaultStore};

use crate::crypto::CryptoCtx;
use crate::error::{NatsKvVaultError, NatsResult as _};
use crate::slot::RotationSlot;

// ── NatsKvVault ───────────────────────────────────────────────────────────────

/// [`VaultStore`] backed by NATS JetStream Key-Value with AES-256-GCM encryption.
///
/// Resolves tokens in nanoseconds from an in-process [`DashMap`] cache that is
/// kept in sync by a background watcher. The watcher uses [`kv::Store::watch_with_history`]
/// so it covers any writes that race with the initial load.
///
/// NATS never stores plaintext — every value is encrypted with AES-256-GCM
/// before being written to the KV bucket.
pub struct NatsKvVault {
    cache:    Arc<DashMap<String, RotationSlot>>,
    kv:       kv::Store,
    ready:    watch::Receiver<bool>,
    crypto:   Arc<CryptoCtx>,
    _watcher: JoinHandle<()>,
}

impl NatsKvVault {
    /// Create a new vault backed by `kv`.
    ///
    /// 1. Loads all existing keys from NATS KV into the in-process cache.
    /// 2. Signals ready (resolve() is now usable).
    /// 3. Starts a background watcher that keeps the cache up to date.
    pub async fn new(
        kv: kv::Store,
        crypto: Arc<CryptoCtx>,
        grace_period: Duration,
    ) -> Result<Self, NatsKvVaultError> {
        let cache = Arc::new(DashMap::new());
        let (ready_tx, ready_rx) = watch::channel(false);

        // Phase 1: load existing keys synchronously
        {
            let mut keys = kv.keys().await.nats_err()?;
            while let Some(key) = keys.next().await {
                let key = key.nats_err()?;
                if let Some(bytes) = kv.get(&key).await.nats_err()? {
                    match crypto.decrypt(&bytes) {
                        Ok(plaintext) => match String::from_utf8(plaintext) {
                            Ok(s) => { cache.insert(key, RotationSlot::new(s)); }
                            Err(e) => tracing::warn!(key, error = %e, "vault: invalid UTF-8, skipping"),
                        },
                        Err(e) => tracing::warn!(key, error = %e, "vault: decrypt failed on startup, skipping"),
                    }
                }
            }
        }

        // Signal ready — resolve() is usable from this point
        ready_tx.send(true).ok();

        // Phase 2: background watcher for ongoing changes
        // Uses watch_with_history to also catch any writes that raced the initial load
        let cache_clone  = Arc::clone(&cache);
        let crypto_clone = Arc::clone(&crypto);
        let kv_clone     = kv.clone();

        let watcher = tokio::spawn(async move {
            let mut watcher = match kv_clone.watch_with_history(">").await {
                Ok(w)  => w,
                Err(e) => { tracing::error!(error = %e, "vault: watcher failed to start"); return; }
            };

            while let Some(entry) = watcher.next().await {
                match entry {
                    Ok(e) => apply_entry(&cache_clone, &crypto_clone, e, grace_period),
                    Err(e) => tracing::warn!(error = %e, "vault: watcher stream error"),
                }
            }
        });

        Ok(Self {
            cache,
            kv,
            ready: ready_rx,
            crypto,
            _watcher: watcher,
        })
    }

    /// Return `(current, previous)` for a token.
    ///
    /// Used by the proxy worker for graceful fallback during key rotation:
    /// if the upstream API rejects `current` with 401, the worker retries
    /// with `previous` (if still within the grace period).
    pub fn slot(&self, token: &ApiKeyToken) -> Option<(String, Option<String>)> {
        self.cache.get(token.as_str()).map(|s| {
            (
                s.current.clone(),
                s.valid_previous().map(String::from),
            )
        })
    }
}

// ── VaultStore impl ───────────────────────────────────────────────────────────

impl VaultStore for NatsKvVault {
    type Error = NatsKvVaultError;

    async fn store(&self, token: &ApiKeyToken, plaintext: &str) -> Result<(), Self::Error> {
        let ciphertext = self.crypto.encrypt(plaintext.as_bytes())?;
        self.kv.put(token.as_str(), ciphertext.into()).await.nats_err()?;
        Ok(())
    }

    async fn resolve(&self, token: &ApiKeyToken) -> Result<Option<String>, Self::Error> {
        wait_ready(&self.ready).await?;
        Ok(self.cache.get(token.as_str()).map(|s| s.current.clone()))
    }

    async fn revoke(&self, token: &ApiKeyToken) -> Result<(), Self::Error> {
        // kv.delete() publishes a tombstone regardless — safe for non-existent keys
        match self.kv.delete(token.as_str()).await {
            Ok(_)  => Ok(()),
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

    async fn resolve_with_previous(
        &self,
        token: &ApiKeyToken,
    ) -> Result<(Option<String>, Option<String>), NatsKvVaultError> {
        wait_ready(&self.ready).await?;
        Ok(match self.cache.get(token.as_str()) {
            Some(slot) => (Some(slot.current.clone()), slot.valid_previous().map(String::from)),
            None => (None, None),
        })
    }

    // rotate() uses the default impl (delegates to store()), which re-encrypts
    // and overwrites. The watcher picks up the Put and updates the RotationSlot,
    // saving the old value as `previous` for the grace period.
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn apply_entry(
    cache:        &DashMap<String, RotationSlot>,
    crypto:       &CryptoCtx,
    entry:        kv::Entry,
    grace_period: Duration,
) {
    match entry.operation {
        kv::Operation::Put => {
            match crypto.decrypt(&entry.value) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(plaintext) => {
                        cache
                            .entry(entry.key)
                            .and_modify(|slot| {
                                // Only rotate the slot when the value actually changed
                                if slot.current != plaintext {
                                    let old = std::mem::replace(&mut slot.current, plaintext.clone());
                                    slot.previous = Some((old, Instant::now() + grace_period));
                                }
                            })
                            .or_insert_with(|| RotationSlot::new(plaintext));
                    }
                    Err(e) => tracing::warn!(key = entry.key, error = %e, "vault: invalid UTF-8 in watcher"),
                },
                Err(e) => tracing::warn!(key = entry.key, error = %e, "vault: decrypt failed in watcher"),
            }
        }
        kv::Operation::Delete | kv::Operation::Purge => {
            cache.remove(&entry.key);
        }
    }
}

async fn wait_ready(rx: &watch::Receiver<bool>) -> Result<(), NatsKvVaultError> {
    if *rx.borrow() {
        return Ok(());
    }
    let mut rx = rx.clone();
    rx.changed()
        .await
        .map_err(|_| NatsKvVaultError::Shutdown)
}
