use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream::kv;
use dashmap::DashMap;
use futures_util::StreamExt as _;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use trogon_vault::{ApiKeyToken, VaultStore};

use crate::audit::Audit;
use crate::crypto::CryptoCtx;
use crate::error::{NatsKvVaultError, NatsResult as _};
use crate::kv_ops::KvOps;
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
pub struct NatsKvVault<K: KvOps = kv::Store> {
    cache:    Arc<DashMap<String, RotationSlot>>,
    kv:       Arc<K>,
    ready:    watch::Receiver<bool>,
    crypto:   Arc<CryptoCtx>,
    audit:    Option<Arc<dyn Audit>>,
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
        let kv_for_watcher = kv.clone();  // kv::Store is cheap to clone (Arc-backed)

        let watcher = tokio::spawn(async move {
            let mut watcher = match kv_for_watcher.watch_with_history(">").await {
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
            kv:    Arc::new(kv),
            ready: ready_rx,
            crypto,
            audit: None,
            _watcher: watcher,
        })
    }

}

impl<K: KvOps> NatsKvVault<K> {
    /// Attach an [`Audit`] implementation so every vault operation emits an audit event.
    pub fn with_audit(mut self, publisher: Arc<dyn Audit>) -> Self {
        self.audit = Some(publisher);
        self
    }

    /// Return `(current, previous)` for a token.
    pub fn slot(&self, token: &ApiKeyToken) -> Option<(String, Option<String>)> {
        self.cache.get(token.as_str()).map(|s| {
            (s.current.clone(), s.valid_previous().map(String::from))
        })
    }
}

// ── VaultStore impl ───────────────────────────────────────────────────────────

impl<K: KvOps> VaultStore for NatsKvVault<K> {
    type Error = NatsKvVaultError;

    async fn store(&self, token: &ApiKeyToken, plaintext: &str) -> Result<(), Self::Error> {
        let ciphertext = self.crypto.encrypt(plaintext.as_bytes())?;
        self.kv.kv_put(token.as_str(), ciphertext.into()).await?;
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
        self.kv.kv_delete(token.as_str()).await?;
        if let Some(ref audit) = self.audit {
            audit.publish_revoke(token.as_str(), "system");
        }
        Ok(())
    }

    async fn rotate(&self, token: &ApiKeyToken, new_plaintext: &str) -> Result<(), Self::Error> {
        let ciphertext = self.crypto.encrypt(new_plaintext.as_bytes())?;
        self.kv.kv_put(token.as_str(), ciphertext.into()).await?;
        if let Some(ref audit) = self.audit {
            audit.publish_rotate(token.as_str(), "system");
        }
        Ok(())
    }

    async fn resolve_with_previous(
        &self,
        token: &ApiKeyToken,
    ) -> Result<(Option<String>, Option<String>), Self::Error> {
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

// ── Test constructor ──────────────────────────────────────────────────────────

#[cfg(test)]
impl NatsKvVault<crate::kv_ops::InMemoryKv> {
    /// Construct a vault backed by an [`InMemoryKv`] mock with a pre-seeded cache.
    ///
    /// The `ready` signal is immediately true; no background watcher is spawned.
    /// Use this to unit-test [`VaultStore`] methods without a real NATS server.
    pub(crate) fn new_for_test(
        kv:           crate::kv_ops::InMemoryKv,
        crypto:       CryptoCtx,
        initial_slots: Vec<(&str, crate::slot::RotationSlot)>,
    ) -> Self {
        let cache = Arc::new(DashMap::new());
        for (key, slot) in initial_slots {
            cache.insert(key.to_string(), slot);
        }
        let (ready_tx, ready_rx) = watch::channel(true);
        drop(ready_tx); // value is already true; sender not needed
        let dummy = tokio::spawn(async {});
        Self {
            cache,
            kv:       Arc::new(kv),
            ready:    ready_rx,
            crypto:   Arc::new(crypto),
            audit:    None,
            _watcher: dummy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::jetstream::kv;
    use bytes::Bytes;

    use crate::crypto::CryptoCtx;
    use crate::kv_ops::InMemoryKv;

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

    // ── VaultStore methods ────────────────────────────────────────────────────

    fn tok(s: &str) -> trogon_vault::ApiKeyToken {
        trogon_vault::ApiKeyToken::new(s).unwrap()
    }

    fn vault_with_cache(slots: Vec<(&str, &str)>) -> (NatsKvVault<InMemoryKv>, InMemoryKv) {
        let kv = InMemoryKv::new();
        let initial = slots
            .into_iter()
            .map(|(k, v)| (k, RotationSlot::new(v.to_string())))
            .collect();
        let v = NatsKvVault::new_for_test(kv.clone(), crypto(), initial);
        (v, kv)
    }

    fn empty_vault() -> (NatsKvVault<InMemoryKv>, InMemoryKv) {
        vault_with_cache(vec![])
    }

    #[tokio::test]
    async fn store_writes_encrypted_value_to_kv() {
        let (vault, kv) = empty_vault();
        vault.store(&tok("tok_stripe_prod_abc1"), "sk_live_secret").await.unwrap();

        let raw = kv.get("tok_stripe_prod_abc1").expect("kv must have entry after store");
        // Must be ciphertext, not plaintext
        assert_ne!(raw.as_ref(), b"sk_live_secret", "value in KV must be encrypted");
        // Must be decryptable back to the original plaintext
        let decrypted = crypto().decrypt(&raw).unwrap();
        assert_eq!(String::from_utf8(decrypted).unwrap(), "sk_live_secret");
    }

    #[tokio::test]
    async fn resolve_returns_value_from_cache() {
        let (vault, _kv) = vault_with_cache(vec![("tok_stripe_prod_abc1", "sk_live_abc")]);
        let result = vault.resolve(&tok("tok_stripe_prod_abc1")).await.unwrap();
        assert_eq!(result.as_deref(), Some("sk_live_abc"));
    }

    #[tokio::test]
    async fn resolve_returns_none_for_unknown_token() {
        let (vault, _kv) = empty_vault();
        let result = vault.resolve(&tok("tok_stripe_prod_abc1")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn revoke_deletes_key_from_kv() {
        let kv = InMemoryKv::new();
        let ct = crypto().encrypt(b"sk_live_abc").unwrap();
        kv.kv_put("tok_stripe_prod_abc1", ct.into()).await.unwrap();

        let vault = NatsKvVault::new_for_test(kv.clone(), crypto(), vec![]);
        vault.revoke(&tok("tok_stripe_prod_abc1")).await.unwrap();

        assert!(!kv.contains("tok_stripe_prod_abc1"), "kv entry must be removed after revoke");
    }

    #[tokio::test]
    async fn revoke_nonexistent_token_is_ok() {
        let (vault, _kv) = empty_vault();
        assert!(vault.revoke(&tok("tok_stripe_prod_abc1")).await.is_ok());
    }

    #[tokio::test]
    async fn rotate_writes_new_encrypted_value_to_kv() {
        let (vault, kv) = vault_with_cache(vec![("tok_stripe_prod_abc1", "old_key")]);
        vault.rotate(&tok("tok_stripe_prod_abc1"), "new_key").await.unwrap();

        let raw = kv.get("tok_stripe_prod_abc1").expect("kv must have entry after rotate");
        let decrypted = crypto().decrypt(&raw).unwrap();
        assert_eq!(String::from_utf8(decrypted).unwrap(), "new_key");
    }

    #[tokio::test]
    async fn resolve_with_previous_returns_current_and_none_when_no_previous() {
        let (vault, _kv) = vault_with_cache(vec![("tok_stripe_prod_abc1", "current_key")]);
        let (current, previous) = vault.resolve_with_previous(&tok("tok_stripe_prod_abc1")).await.unwrap();
        assert_eq!(current.as_deref(), Some("current_key"));
        assert!(previous.is_none());
    }

    #[tokio::test]
    async fn resolve_with_previous_returns_both_during_grace_period() {
        // Call crypto() BEFORE capturing Instant::now(): argon2id is slow and
        // CPU-contended in parallel test runs, so the expiry must be set after
        // any expensive setup to avoid a race with the grace period.
        let kv     = InMemoryKv::new();
        let crypto = crypto();
        let mut slot = RotationSlot::new("new_key".to_string());
        slot.previous = Some(("old_key".to_string(), std::time::Instant::now() + Duration::from_secs(300)));
        let vault = NatsKvVault::new_for_test(kv, crypto, vec![("tok_stripe_prod_abc1", slot)]);

        let (current, previous) = vault.resolve_with_previous(&tok("tok_stripe_prod_abc1")).await.unwrap();
        assert_eq!(current.as_deref(),  Some("new_key"));
        assert_eq!(previous.as_deref(), Some("old_key"));
    }

    #[tokio::test]
    async fn resolve_with_previous_returns_none_for_unknown_token() {
        let (vault, _kv) = empty_vault();
        let (current, previous) = vault.resolve_with_previous(&tok("tok_stripe_prod_abc1")).await.unwrap();
        assert!(current.is_none());
        assert!(previous.is_none());
    }
}
