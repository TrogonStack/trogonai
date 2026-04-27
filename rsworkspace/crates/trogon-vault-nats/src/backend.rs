use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream::kv;
use dashmap::DashMap;
use futures_util::StreamExt as _;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use trogon_vault::{ApiKeyToken, VaultStore};

use crate::audit::AuditPublisher;
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
///
/// Attach an [`AuditPublisher`] via [`with_audit`](NatsKvVault::with_audit) to
/// emit fire-and-forget audit events for every vault operation.
pub struct NatsKvVault {
    cache:    Arc<DashMap<String, RotationSlot>>,
    kv:       kv::Store,
    ready:    watch::Receiver<bool>,
    crypto:   Arc<CryptoCtx>,
    audit:    Option<Arc<AuditPublisher>>,
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
            audit: None,
            _watcher: watcher,
        })
    }

    /// Attach an [`AuditPublisher`] so every vault operation emits an audit event.
    ///
    /// Call [`ensure_audit_stream`](crate::ensure_audit_stream) before using this
    /// so the `VAULT_AUDIT` stream exists to receive the events.
    pub fn with_audit(mut self, publisher: Arc<AuditPublisher>) -> Self {
        self.audit = Some(publisher);
        self
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
        if let Some(ref audit) = self.audit {
            audit.publish_store(token.as_str(), "system");
        }
        Ok(())
    }

    async fn resolve(&self, token: &ApiKeyToken) -> Result<Option<String>, Self::Error> {
        wait_ready(&self.ready).await?;
        let t0 = std::time::Instant::now();
        let result = self.cache.get(token.as_str()).map(|s| s.current.clone());
        if let Some(ref audit) = self.audit {
            audit.publish_resolve(token.as_str(), result.is_some(), t0.elapsed().as_micros() as u64);
        }
        Ok(result)
    }

    async fn revoke(&self, token: &ApiKeyToken) -> Result<(), Self::Error> {
        // kv.delete() publishes a tombstone regardless — safe for non-existent keys
        match self.kv.delete(token.as_str()).await {
            Ok(_)  => {
                if let Some(ref audit) = self.audit {
                    audit.publish_revoke(token.as_str(), "system");
                }
                Ok(())
            }
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

    async fn rotate(&self, token: &ApiKeyToken, new_plaintext: &str) -> Result<(), Self::Error> {
        let ciphertext = self.crypto.encrypt(new_plaintext.as_bytes())?;
        self.kv.put(token.as_str(), ciphertext.into()).await.nats_err()?;
        if let Some(ref audit) = self.audit {
            audit.publish_rotate(token.as_str(), "system");
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::jetstream::kv;
    use bytes::Bytes;

    use crate::crypto::CryptoCtx;

    fn crypto() -> CryptoCtx {
        CryptoCtx::derive(b"test-password", b"test-salt-16byte").unwrap()
    }

    fn put_entry(key: &str, value: Bytes) -> kv::Entry {
        kv::Entry {
            bucket:       "vault_test".into(),
            key:          key.into(),
            value,
            revision:     1,
            delta:        0,
            created:      time::OffsetDateTime::UNIX_EPOCH,
            operation:    kv::Operation::Put,
            seen_current: false,
        }
    }

    fn delete_entry(key: &str, op: kv::Operation) -> kv::Entry {
        kv::Entry {
            bucket:       "vault_test".into(),
            key:          key.into(),
            value:        Bytes::new(),
            revision:     2,
            delta:        0,
            created:      time::OffsetDateTime::UNIX_EPOCH,
            operation:    op,
            seen_current: false,
        }
    }

    // ── apply_entry ───────────────────────────────────────────────────────────

    #[test]
    fn put_inserts_new_slot() {
        let cache  = DashMap::new();
        let crypto = crypto();
        let ct     = crypto.encrypt(b"sk_live_abc").unwrap();

        apply_entry(&cache, &crypto, put_entry("tok_stripe_prod_abc", ct.into()), Duration::from_secs(30));

        let slot = cache.get("tok_stripe_prod_abc").unwrap();
        assert_eq!(slot.current, "sk_live_abc");
        assert!(slot.previous.is_none());
    }

    #[test]
    fn put_different_value_rotates_slot() {
        let cache  = DashMap::new();
        let crypto = crypto();
        cache.insert("tok_stripe_prod_abc".to_string(), RotationSlot::new("old_key".to_string()));

        let ct = crypto.encrypt(b"new_key").unwrap();
        apply_entry(&cache, &crypto, put_entry("tok_stripe_prod_abc", ct.into()), Duration::from_secs(30));

        let slot = cache.get("tok_stripe_prod_abc").unwrap();
        assert_eq!(slot.current, "new_key");
        let (prev, _) = slot.previous.as_ref().expect("previous must be set after rotation");
        assert_eq!(prev, "old_key");
    }

    #[test]
    fn put_same_value_does_not_rotate() {
        let cache  = DashMap::new();
        let crypto = crypto();
        cache.insert("tok_stripe_prod_abc".to_string(), RotationSlot::new("same_key".to_string()));

        let ct = crypto.encrypt(b"same_key").unwrap();
        apply_entry(&cache, &crypto, put_entry("tok_stripe_prod_abc", ct.into()), Duration::from_secs(30));

        let slot = cache.get("tok_stripe_prod_abc").unwrap();
        assert_eq!(slot.current, "same_key");
        assert!(slot.previous.is_none(), "same value must not trigger rotation");
    }

    #[test]
    fn delete_removes_entry_from_cache() {
        let cache  = DashMap::new();
        let crypto = crypto();
        cache.insert("tok_stripe_prod_abc".to_string(), RotationSlot::new("some_key".to_string()));

        apply_entry(&cache, &crypto, delete_entry("tok_stripe_prod_abc", kv::Operation::Delete), Duration::from_secs(30));

        assert!(cache.get("tok_stripe_prod_abc").is_none());
    }

    #[test]
    fn purge_removes_entry_from_cache() {
        let cache  = DashMap::new();
        let crypto = crypto();
        cache.insert("tok_stripe_prod_abc".to_string(), RotationSlot::new("some_key".to_string()));

        apply_entry(&cache, &crypto, delete_entry("tok_stripe_prod_abc", kv::Operation::Purge), Duration::from_secs(30));

        assert!(cache.get("tok_stripe_prod_abc").is_none());
    }

    #[test]
    fn invalid_ciphertext_skips_without_crash() {
        let cache  = DashMap::new();
        let crypto = crypto();

        apply_entry(&cache, &crypto, put_entry("tok_stripe_prod_abc", Bytes::from_static(b"not_valid_ciphertext")), Duration::from_secs(30));

        assert!(cache.get("tok_stripe_prod_abc").is_none(), "decrypt failure must not insert entry");
    }

    // ── wait_ready ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn wait_ready_returns_immediately_when_already_true() {
        let (_tx, rx) = watch::channel(true);
        assert!(wait_ready(&rx).await.is_ok());
    }

    #[tokio::test]
    async fn wait_ready_errors_when_sender_is_dropped_before_signal() {
        let (tx, rx) = watch::channel(false);
        drop(tx);
        assert!(matches!(wait_ready(&rx).await, Err(NatsKvVaultError::Shutdown)));
    }

    #[tokio::test]
    async fn wait_ready_unblocks_when_sender_fires() {
        let (tx, rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            tx.send(true).ok();
        });
        assert!(wait_ready(&rx).await.is_ok());
    }
}
