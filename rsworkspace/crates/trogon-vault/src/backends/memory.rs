//! In-memory [`VaultStore`] backend backed by `Arc<Mutex<HashMap>>`.
//!
//! Suitable for tests and local development. Do **not** use in production —
//! it holds plaintext API keys in process memory with no persistence.

use crate::token::ApiKeyToken;
use crate::vault::VaultStore;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// Error type for [`MemoryVault`].
#[derive(Debug)]
pub struct MemoryVaultError(String);

impl fmt::Display for MemoryVaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MemoryVault error: {}", self.0)
    }
}

impl std::error::Error for MemoryVaultError {}

/// Thread-safe in-memory token store.
///
/// Backed by `Arc<Mutex<HashMap<String, String>>>` so it is `Clone`,
/// `Send`, and `Sync` — safe to share across `tokio` tasks.
#[derive(Clone, Default)]
pub struct MemoryVault {
    inner: Arc<Mutex<HashMap<String, String>>>,
}

impl MemoryVault {
    /// Create a new, empty vault.
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience method for seeding the vault in tests without `await`.
    pub fn insert(&self, token: &ApiKeyToken, plaintext: impl Into<String>) {
        self.inner
            .lock()
            .unwrap()
            .insert(token.as_str().to_string(), plaintext.into());
    }

    /// Return the number of stored tokens.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    /// Return `true` if the vault contains no tokens.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }
}

impl VaultStore for MemoryVault {
    type Error = MemoryVaultError;

    async fn store(&self, token: &ApiKeyToken, plaintext: &str) -> Result<(), Self::Error> {
        self.inner
            .lock()
            .unwrap()
            .insert(token.as_str().to_string(), plaintext.to_string());
        Ok(())
    }

    async fn resolve(&self, token: &ApiKeyToken) -> Result<Option<String>, Self::Error> {
        let guard = self.inner.lock().unwrap();
        Ok(guard.get(token.as_str()).cloned())
    }

    async fn revoke(&self, token: &ApiKeyToken) -> Result<(), Self::Error> {
        self.inner.lock().unwrap().remove(token.as_str());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::ApiKeyToken;

    fn tok(s: &str) -> ApiKeyToken {
        ApiKeyToken::new(s).unwrap()
    }

    #[tokio::test]
    async fn store_then_resolve_returns_plaintext() {
        let vault = MemoryVault::new();
        let token = tok("tok_anthropic_prod_abc123");
        vault.store(&token, "sk-ant-realkey").await.unwrap();
        let result = vault.resolve(&token).await.unwrap();
        assert_eq!(result, Some("sk-ant-realkey".to_string()));
    }

    #[tokio::test]
    async fn resolve_unknown_token_returns_none() {
        let vault = MemoryVault::new();
        let token = tok("tok_openai_prod_xyz789");
        let result = vault.resolve(&token).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn revoke_removes_token() {
        let vault = MemoryVault::new();
        let token = tok("tok_anthropic_staging_aa1111");
        vault.store(&token, "sk-secret").await.unwrap();
        vault.revoke(&token).await.unwrap();
        let result = vault.resolve(&token).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn store_overwrites_existing() {
        let vault = MemoryVault::new();
        let token = tok("tok_openai_dev_zz0001");
        vault.store(&token, "key-v1").await.unwrap();
        vault.store(&token, "key-v2").await.unwrap();
        let result = vault.resolve(&token).await.unwrap();
        assert_eq!(result, Some("key-v2".to_string()));
    }

    #[test]
    fn insert_helper_stores_synchronously() {
        let vault = MemoryVault::new();
        let token = tok("tok_gemini_test_ff9999");
        vault.insert(&token, "gemini-realkey");
        // Verify with sync access
        let guard = vault.inner.lock().unwrap();
        assert_eq!(guard.get("tok_gemini_test_ff9999").map(|s| s.as_str()), Some("gemini-realkey"));
    }

    #[test]
    fn len_and_is_empty() {
        let vault = MemoryVault::new();
        assert!(vault.is_empty());
        assert_eq!(vault.len(), 0);

        let token = tok("tok_anthropic_prod_aa0001");
        vault.insert(&token, "key");
        assert!(!vault.is_empty());
        assert_eq!(vault.len(), 1);
    }

    #[test]
    fn clone_shares_state() {
        let vault = MemoryVault::new();
        let token = tok("tok_openai_prod_bb2222");
        vault.insert(&token, "shared-key");

        let clone = vault.clone();
        let guard = clone.inner.lock().unwrap();
        assert_eq!(guard.get("tok_openai_prod_bb2222").map(|s| s.as_str()), Some("shared-key"));
    }

    #[tokio::test]
    async fn rotate_replaces_existing_plaintext() {
        let vault = MemoryVault::new();
        let token = tok("tok_anthropic_prod_abc123");
        vault.store(&token, "key-v1").await.unwrap();
        vault.rotate(&token, "key-v2").await.unwrap();
        let result = vault.resolve(&token).await.unwrap();
        assert_eq!(result, Some("key-v2".to_string()));
    }

    #[test]
    fn error_display() {
        let err = MemoryVaultError("something went wrong".to_string());
        assert!(err.to_string().contains("something went wrong"));
    }
}
