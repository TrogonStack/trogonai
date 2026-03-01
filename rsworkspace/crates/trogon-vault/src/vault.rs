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
}
