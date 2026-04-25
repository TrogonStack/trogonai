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
