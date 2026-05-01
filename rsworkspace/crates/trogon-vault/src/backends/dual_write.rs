//! Composition layer: primary (source of truth) + cache (fast read path).
//!
//! [`DualWriteVault`] implements [`VaultStore`] by writing to both backends on
//! every mutation and reading from the cache first on every lookup.
//!
//! # Semantics
//!
//! - **store / rotate / revoke**: primary is always written first. If the primary
//!   write succeeds, the cache write is attempted; a cache failure is logged as a
//!   warning but does NOT fail the operation — the primary is the source of truth.
//! - **resolve**: cache is consulted first. On cache miss or cache error, falls
//!   back to the primary. Cache errors are logged at `warn` level.
//! - **resolve_with_previous**: cache is consulted first (it may hold a rotation
//!   slot with the previous key). Falls back to `(primary.resolve(), None)` on
//!   cache miss or error.
//!
//! # Typical wiring
//!
//! ```rust,ignore
//! let vault = DualWriteVault::new(infisical_store, nats_kv_vault);
//! ```

use crate::token::ApiKeyToken;
use crate::vault::VaultStore;

// ── DualWriteVault ────────────────────────────────────────────────────────────

/// A [`VaultStore`] that writes to a primary (source of truth) and a cache,
/// and reads from the cache first.
pub struct DualWriteVault<P, C> {
    primary: P,
    cache:   C,
}

impl<P, C> DualWriteVault<P, C> {
    pub fn new(primary: P, cache: C) -> Self {
        Self { primary, cache }
    }

    /// Destructure into the inner primary and cache stores.
    pub fn into_parts(self) -> (P, C) {
        (self.primary, self.cache)
    }
}

// ── VaultStore impl ───────────────────────────────────────────────────────────

impl<P, C> VaultStore for DualWriteVault<P, C>
where
    P: VaultStore,
    P::Error: std::fmt::Display,
    C: VaultStore,
    C::Error: std::fmt::Display,
{
    type Error = P::Error;

    async fn store(&self, token: &ApiKeyToken, plaintext: &str) -> Result<(), Self::Error> {
        self.primary.store(token, plaintext).await?;
        if let Err(e) = self.cache.store(token, plaintext).await {
            tracing::warn!(token = token.as_str(), error = %e, "dual-write: cache store failed");
        }
        Ok(())
    }

    async fn resolve(&self, token: &ApiKeyToken) -> Result<Option<String>, Self::Error> {
        match self.cache.resolve(token).await {
            Ok(Some(v)) => return Ok(Some(v)),
            Ok(None)    => {}
            Err(e)      => tracing::warn!(token = token.as_str(), error = %e, "dual-write: cache resolve failed, falling back to primary"),
        }
        self.primary.resolve(token).await
    }

    async fn revoke(&self, token: &ApiKeyToken) -> Result<(), Self::Error> {
        self.primary.revoke(token).await?;
        if let Err(e) = self.cache.revoke(token).await {
            tracing::warn!(token = token.as_str(), error = %e, "dual-write: cache revoke failed");
        }
        Ok(())
    }

    async fn rotate(&self, token: &ApiKeyToken, new_plaintext: &str) -> Result<(), Self::Error> {
        self.primary.rotate(token, new_plaintext).await?;
        if let Err(e) = self.cache.rotate(token, new_plaintext).await {
            tracing::warn!(token = token.as_str(), error = %e, "dual-write: cache rotate failed");
        }
        Ok(())
    }

    async fn resolve_with_previous(
        &self,
        token: &ApiKeyToken,
    ) -> Result<(Option<String>, Option<String>), Self::Error> {
        match self.cache.resolve_with_previous(token).await {
            Ok((Some(current), prev)) => return Ok((Some(current), prev)),
            Ok((None, _))             => {}
            Err(e) => tracing::warn!(token = token.as_str(), error = %e, "dual-write: cache resolve_with_previous failed, falling back to primary"),
        }
        Ok((self.primary.resolve(token).await?, None))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::memory::MemoryVault;

    fn tok(s: &str) -> ApiKeyToken {
        ApiKeyToken::new(s).unwrap()
    }

    fn dual() -> DualWriteVault<MemoryVault, MemoryVault> {
        DualWriteVault::new(MemoryVault::new(), MemoryVault::new())
    }

    #[tokio::test]
    async fn store_writes_to_both() {
        let v = dual();
        let t = tok("tok_anthropic_prod_abc123");
        v.store(&t, "sk-key").await.unwrap();

        assert_eq!(v.primary.resolve(&t).await.unwrap(), Some("sk-key".into()));
        assert_eq!(v.cache.resolve(&t).await.unwrap(),   Some("sk-key".into()));
    }

    #[tokio::test]
    async fn resolve_returns_cache_hit_without_hitting_primary() {
        let v = dual();
        let t = tok("tok_anthropic_prod_abc123");

        // seed only the cache
        v.cache.store(&t, "cache-only-key").await.unwrap();

        let result = v.resolve(&t).await.unwrap();
        assert_eq!(result, Some("cache-only-key".into()));
    }

    #[tokio::test]
    async fn resolve_falls_back_to_primary_on_cache_miss() {
        let v = dual();
        let t = tok("tok_anthropic_prod_abc123");

        // seed only the primary
        v.primary.store(&t, "primary-key").await.unwrap();

        let result = v.resolve(&t).await.unwrap();
        assert_eq!(result, Some("primary-key".into()));
    }

    #[tokio::test]
    async fn revoke_removes_from_both() {
        let v = dual();
        let t = tok("tok_anthropic_prod_abc123");

        v.store(&t, "sk-key").await.unwrap();
        v.revoke(&t).await.unwrap();

        assert_eq!(v.primary.resolve(&t).await.unwrap(), None);
        assert_eq!(v.cache.resolve(&t).await.unwrap(),   None);
    }

    #[tokio::test]
    async fn rotate_updates_both() {
        let v = dual();
        let t = tok("tok_anthropic_prod_abc123");

        v.store(&t, "v1").await.unwrap();
        v.rotate(&t, "v2").await.unwrap();

        assert_eq!(v.primary.resolve(&t).await.unwrap(), Some("v2".into()));
        assert_eq!(v.cache.resolve(&t).await.unwrap(),   Some("v2".into()));
    }

    #[tokio::test]
    async fn resolve_returns_none_when_neither_has_token() {
        let v = dual();
        let t = tok("tok_anthropic_prod_abc123");
        assert_eq!(v.resolve(&t).await.unwrap(), None);
    }

    // ── FailingVault ──────────────────────────────────────────────────────────

    #[derive(Debug)]
    struct AlwaysErr;

    impl std::fmt::Display for AlwaysErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "simulated vault failure")
        }
    }

    impl std::error::Error for AlwaysErr {}

    struct FailingVault;

    impl VaultStore for FailingVault {
        type Error = AlwaysErr;
        async fn store(&self, _: &ApiKeyToken, _: &str) -> Result<(), AlwaysErr> { Err(AlwaysErr) }
        async fn resolve(&self, _: &ApiKeyToken) -> Result<Option<String>, AlwaysErr> { Err(AlwaysErr) }
        async fn revoke(&self, _: &ApiKeyToken) -> Result<(), AlwaysErr> { Err(AlwaysErr) }
    }

    // ── Primary-failure / cache-failure tests ─────────────────────────────────

    /// When the primary fails, store() must propagate the error and the cache
    /// must remain empty — no partial write should occur.
    #[tokio::test]
    async fn primary_failure_propagates_error_and_cache_not_written() {
        let cache = MemoryVault::new();
        let cache_check = cache.clone(); // shares the same Arc<Mutex<HashMap>>
        let vault = DualWriteVault::new(FailingVault, cache);
        let t = tok("tok_anthropic_prod_abc123");

        let result = vault.store(&t, "sk-key").await;
        assert!(result.is_err(), "primary failure must propagate as Err");
        assert_eq!(
            cache_check.resolve(&t).await.unwrap(),
            None,
            "cache must not be written when primary fails"
        );
    }

    /// When the cache fails on store(), the operation must still succeed —
    /// the primary is the source of truth and cache failures are non-fatal.
    #[tokio::test]
    async fn cache_failure_on_store_is_silently_ignored() {
        let vault = DualWriteVault::new(MemoryVault::new(), FailingVault);
        let t = tok("tok_anthropic_prod_abc123");

        let result = vault.store(&t, "sk-key").await;
        assert!(result.is_ok(), "cache failure on store must be silently ignored");

        // Primary holds the value; FailingVault as cache means resolve falls
        // through to primary and returns it.
        let resolved = vault.resolve(&t).await.unwrap();
        assert_eq!(resolved, Some("sk-key".to_string()));
    }

    /// When the cache errors on resolve(), DualWriteVault must fall back to
    /// the primary and return its value without surfacing the cache error.
    #[tokio::test]
    async fn resolve_falls_back_to_primary_on_cache_error() {
        let primary = MemoryVault::new();
        let t = tok("tok_anthropic_prod_abc123");
        primary.store(&t, "sk-primary").await.unwrap();

        let vault = DualWriteVault::new(primary, FailingVault);
        let resolved = vault.resolve(&t).await.unwrap();
        assert_eq!(
            resolved,
            Some("sk-primary".to_string()),
            "DualWriteVault must fall back to primary when cache errors on resolve"
        );
    }

    #[tokio::test]
    async fn into_parts_returns_inner_stores() {
        let (primary, cache) = DualWriteVault::new(MemoryVault::new(), MemoryVault::new()).into_parts();
        let t = tok("tok_anthropic_prod_abc123");
        primary.store(&t, "pk").await.unwrap();
        cache.store(&t, "ck").await.unwrap();
        assert_eq!(primary.resolve(&t).await.unwrap(), Some("pk".into()));
        assert_eq!(cache.resolve(&t).await.unwrap(),   Some("ck".into()));
    }
}
