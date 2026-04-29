//! [`VaultStore`] trait — the core abstraction for token ↔ real-key mappings.

use crate::token::ApiKeyToken;

/// Trait for backends that store and resolve proxy tokens.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// async tasks (e.g. wrapped in `Arc<dyn VaultStore>`).
pub trait VaultStore: Send + Sync {
    /// The error type returned by all vault operations.
    type Error: std::error::Error + Send + Sync;

    /// Persist a mapping: `token` → `plaintext` (the real API key).
    fn store(
        &self,
        token: &ApiKeyToken,
        plaintext: &str,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    /// Look up the plaintext API key for `token`.
    ///
    /// Returns `Ok(None)` if the token is not known.
    fn resolve(
        &self,
        token: &ApiKeyToken,
    ) -> impl std::future::Future<Output = Result<Option<String>, Self::Error>> + Send;

    /// Remove the token from the store, making it irresolvable.
    fn revoke(
        &self,
        token: &ApiKeyToken,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    /// Rotate the plaintext API key for `token` to `new_plaintext`.
    ///
    /// Semantically distinct from [`store`](VaultStore::store) (initial creation / forced
    /// overwrite). For Vault KV v2, this writes a new version while keeping prior versions in
    /// history; [`resolve`](VaultStore::resolve) always returns the latest. The old key stays
    /// valid at the provider level for the duration of any grace window.
    ///
    /// Default implementation delegates to [`store`](VaultStore::store). Override for two-phase
    /// rotation (write new, verify, then delete old versions).
    fn rotate(
        &self,
        token: &ApiKeyToken,
        new_plaintext: &str,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        self.store(token, new_plaintext)
    }

    /// Look up both the current and previous plaintext API keys for `token`.
    ///
    /// Returns `(current, previous)`. `previous` is `Some` only during a rotation
    /// grace period, allowing a proxy to fall back to the previous key if the upstream
    /// rejects the current one with 401.
    ///
    /// Default: delegates to [`resolve`](VaultStore::resolve) with `previous = None`.
    /// Override in rotation-aware backends (e.g. [`NatsKvVault`]) to expose the slot.
    fn resolve_with_previous(
        &self,
        token: &ApiKeyToken,
    ) -> impl std::future::Future<Output = Result<(Option<String>, Option<String>), Self::Error>> + Send
    {
        let fut = self.resolve(token);
        async move { Ok((fut.await?, None)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::memory::MemoryVault;
    use crate::token::ApiKeyToken;

    fn tok(s: &str) -> ApiKeyToken {
        ApiKeyToken::new(s).unwrap()
    }

    // --- Trait contract tests via MemoryVault ---

    /// Full lifecycle: store → resolve → rotate → revoke → resolve returns None.
    #[tokio::test]
    async fn full_lifecycle() {
        let vault = MemoryVault::new();
        let token = tok("tok_anthropic_prod_abc123");

        vault.store(&token, "sk-ant-v1").await.unwrap();
        assert_eq!(
            vault.resolve(&token).await.unwrap(),
            Some("sk-ant-v1".to_string())
        );

        vault.rotate(&token, "sk-ant-v2").await.unwrap();
        assert_eq!(
            vault.resolve(&token).await.unwrap(),
            Some("sk-ant-v2".to_string())
        );

        vault.revoke(&token).await.unwrap();
        assert_eq!(vault.resolve(&token).await.unwrap(), None);
    }

    /// Implementations are interchangeable via a generic function bound on `VaultStore`.
    #[tokio::test]
    async fn works_via_generic_vault_store_bound() {
        async fn use_vault<V: VaultStore>(vault: &V) {
            let token = tok("tok_openai_staging_xyz789");
            vault.store(&token, "sk-openai-key").await.unwrap();
            let resolved = vault.resolve(&token).await.unwrap();
            assert_eq!(resolved.unwrap(), "sk-openai-key");
        }

        let vault = MemoryVault::new();
        use_vault(&vault).await;
    }

    /// resolve on a token that was never stored returns Ok(None).
    #[tokio::test]
    async fn resolve_nonexistent_returns_none() {
        let vault = MemoryVault::new();
        let token = tok("tok_gemini_dev_zz9999");
        assert_eq!(vault.resolve(&token).await.unwrap(), None);
    }

    /// revoke on a token that was never stored succeeds (idempotent).
    #[tokio::test]
    async fn revoke_nonexistent_is_ok() {
        let vault = MemoryVault::new();
        let token = tok("tok_anthropic_prod_never11");
        assert!(vault.revoke(&token).await.is_ok());
    }

    /// rotate on a token that was never stored creates the entry (delegates to store).
    #[tokio::test]
    async fn rotate_nonexistent_creates_entry() {
        let vault = MemoryVault::new();
        let token = tok("tok_openai_prod_newone1");
        vault.rotate(&token, "sk-new-key").await.unwrap();
        assert_eq!(
            vault.resolve(&token).await.unwrap(),
            Some("sk-new-key".to_string())
        );
    }

    /// store for different tokens does not interfere.
    #[tokio::test]
    async fn multiple_tokens_are_independent() {
        let vault = MemoryVault::new();
        let t1 = tok("tok_anthropic_prod_aaa111");
        let t2 = tok("tok_openai_prod_bbb222");

        vault.store(&t1, "key-for-t1").await.unwrap();
        vault.store(&t2, "key-for-t2").await.unwrap();

        assert_eq!(
            vault.resolve(&t1).await.unwrap(),
            Some("key-for-t1".to_string())
        );
        assert_eq!(
            vault.resolve(&t2).await.unwrap(),
            Some("key-for-t2".to_string())
        );
    }
}
