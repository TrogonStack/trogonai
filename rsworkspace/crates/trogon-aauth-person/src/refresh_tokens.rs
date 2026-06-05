//! OAuth 2.1 refresh tokens with mandatory rotation.
//!
//! Pairs with [`crate::oauth_code`]. The authorization-code grant returns
//! `(access_token, refresh_token)`; clients later call the token endpoint
//! with `grant_type=refresh_token` to mint a fresh access token. Each refresh
//! call **invalidates the prior refresh token** and issues a new one (RFC
//! 6749 §6 + OAuth 2.1 §4.3.1). The store is the trust boundary: a single
//! atomic `take_and_replace` enforces single-use semantics.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GRANT_TYPE_REFRESH_TOKEN: &str = "refresh_token";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshTokenRecord {
    pub token_hash: String,
    pub client_id: String,
    pub principal: String,
    /// Backend (MCP server) the consent applies to. Per Solo.io's
    /// elicitation spec, consent records are keyed on `(sub, backend)`
    /// for the refresh-token lifetime.
    pub backend: String,
    pub scope: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshTokenRequest {
    pub grant_type: String,
    pub refresh_token: String,
    pub client_id: String,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RefreshError {
    #[error("unsupported grant_type `{0}`")]
    UnsupportedGrantType(String),
    #[error("refresh token not found")]
    NotFound,
    #[error("refresh token expired")]
    Expired,
    #[error("refresh token does not match client")]
    ClientMismatch,
    #[error("requested scope exceeds original grant")]
    ScopeExceedsGrant,
    #[error("store: {0}")]
    Store(String),
}

#[derive(Debug)]
pub enum RefreshTokenStoreError {
    Backend(String),
}

impl std::fmt::Display for RefreshTokenStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(m) => write!(f, "refresh token store: {m}"),
        }
    }
}

impl std::error::Error for RefreshTokenStoreError {}

#[async_trait]
pub trait RefreshTokenStore: Send + Sync {
    async fn put(&self, record: RefreshTokenRecord) -> Result<(), RefreshTokenStoreError>;
    /// Atomically read-and-delete by hash. Refresh tokens are single-use.
    async fn take(&self, token_hash: &str) -> Result<Option<RefreshTokenRecord>, RefreshTokenStoreError>;
    /// Look up the active consent for `(principal, backend)` without
    /// invalidating it. Returns the most-recent active record (if any).
    async fn find_active_for_backend(
        &self,
        principal: &str,
        backend: &str,
        now: i64,
    ) -> Result<Option<RefreshTokenRecord>, RefreshTokenStoreError>;
}

#[derive(Default)]
pub struct InMemoryRefreshTokenStore {
    records: Mutex<HashMap<String, RefreshTokenRecord>>,
}

impl InMemoryRefreshTokenStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RefreshTokenStore for InMemoryRefreshTokenStore {
    async fn put(&self, record: RefreshTokenRecord) -> Result<(), RefreshTokenStoreError> {
        let mut guard = self.records.lock().expect("refresh store lock");
        guard.insert(record.token_hash.clone(), record);
        Ok(())
    }

    async fn take(&self, token_hash: &str) -> Result<Option<RefreshTokenRecord>, RefreshTokenStoreError> {
        let mut guard = self.records.lock().expect("refresh store lock");
        Ok(guard.remove(token_hash))
    }

    async fn find_active_for_backend(
        &self,
        principal: &str,
        backend: &str,
        now: i64,
    ) -> Result<Option<RefreshTokenRecord>, RefreshTokenStoreError> {
        let guard = self.records.lock().expect("refresh store lock");
        Ok(guard
            .values()
            .filter(|r| r.principal == principal && r.backend == backend && r.expires_at > now)
            .max_by_key(|r| r.issued_at)
            .cloned())
    }
}

/// Returns the SHA-256 hash of the raw refresh token, URL-safe base64.
/// The store never sees the plaintext, only the hash.
pub fn hash_token(raw: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(raw.as_bytes()))
}

/// Validate the refresh grant against a record fetched from the store.
/// The caller is expected to have `take`-d the record already; this only
/// inspects the validated fields. The caller then `put`s a fresh record.
pub fn validate_refresh_grant(
    req: &RefreshTokenRequest,
    record: &RefreshTokenRecord,
    now: i64,
) -> Result<(), RefreshError> {
    if req.grant_type != GRANT_TYPE_REFRESH_TOKEN {
        return Err(RefreshError::UnsupportedGrantType(req.grant_type.clone()));
    }
    if record.client_id != req.client_id {
        return Err(RefreshError::ClientMismatch);
    }
    if now > record.expires_at {
        return Err(RefreshError::Expired);
    }
    if let Some(requested) = req.scope.as_deref()
        && !requested.is_empty()
        && !scope_is_subset(requested, &record.scope)
    {
        return Err(RefreshError::ScopeExceedsGrant);
    }
    Ok(())
}

fn scope_is_subset(requested: &str, granted: &str) -> bool {
    let granted_set: std::collections::HashSet<&str> = granted.split_whitespace().collect();
    requested
        .split_whitespace()
        .all(|tok| granted_set.contains(tok))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> RefreshTokenRecord {
        RefreshTokenRecord {
            token_hash: hash_token("rt-1"),
            client_id: "cursor-cli".into(),
            principal: "alice".into(),
            backend: "mcp-fileserver".into(),
            scope: "read:tools write:tools".into(),
            issued_at: 0,
            expires_at: 1_000,
        }
    }

    fn sample_request(raw: &str) -> RefreshTokenRequest {
        RefreshTokenRequest {
            grant_type: GRANT_TYPE_REFRESH_TOKEN.into(),
            refresh_token: raw.into(),
            client_id: "cursor-cli".into(),
            scope: None,
        }
    }

    #[test]
    fn hash_is_deterministic_and_url_safe() {
        let hash = hash_token("rt-secret");
        assert!(!hash.contains('+') && !hash.contains('/') && !hash.contains('='));
        assert_eq!(hash, hash_token("rt-secret"));
    }

    #[test]
    fn refresh_happy_path() {
        validate_refresh_grant(&sample_request("rt-1"), &sample_record(), 100).expect("ok");
    }

    #[test]
    fn refresh_rejects_expired() {
        let err = validate_refresh_grant(&sample_request("rt-1"), &sample_record(), 10_000).unwrap_err();
        assert_eq!(err, RefreshError::Expired);
    }

    #[test]
    fn refresh_rejects_client_mismatch() {
        let mut req = sample_request("rt-1");
        req.client_id = "evil-cli".into();
        let err = validate_refresh_grant(&req, &sample_record(), 100).unwrap_err();
        assert_eq!(err, RefreshError::ClientMismatch);
    }

    #[test]
    fn refresh_rejects_scope_escalation() {
        let mut req = sample_request("rt-1");
        req.scope = Some("read:tools admin:secrets".into());
        let err = validate_refresh_grant(&req, &sample_record(), 100).unwrap_err();
        assert_eq!(err, RefreshError::ScopeExceedsGrant);
    }

    #[test]
    fn refresh_allows_scope_narrowing() {
        let mut req = sample_request("rt-1");
        req.scope = Some("read:tools".into());
        validate_refresh_grant(&req, &sample_record(), 100).expect("subset");
    }

    #[test]
    fn refresh_rejects_wrong_grant_type() {
        let mut req = sample_request("rt-1");
        req.grant_type = "authorization_code".into();
        let err = validate_refresh_grant(&req, &sample_record(), 100).unwrap_err();
        assert!(matches!(err, RefreshError::UnsupportedGrantType(_)));
    }

    #[tokio::test]
    async fn store_is_single_use() {
        let store = InMemoryRefreshTokenStore::new();
        let record = sample_record();
        store.put(record.clone()).await.unwrap();
        assert_eq!(store.take(&record.token_hash).await.unwrap(), Some(record.clone()));
        assert_eq!(store.take(&record.token_hash).await.unwrap(), None);
    }

    #[tokio::test]
    async fn find_active_for_backend_returns_latest_unexpired() {
        let store = InMemoryRefreshTokenStore::new();
        let mut older = sample_record();
        older.token_hash = hash_token("rt-old");
        older.issued_at = 10;
        let mut newer = sample_record();
        newer.token_hash = hash_token("rt-new");
        newer.issued_at = 20;
        let mut other_backend = sample_record();
        other_backend.token_hash = hash_token("rt-other");
        other_backend.backend = "mcp-search".into();
        other_backend.issued_at = 30;
        store.put(older).await.unwrap();
        store.put(newer.clone()).await.unwrap();
        store.put(other_backend).await.unwrap();
        let found = store
            .find_active_for_backend("alice", "mcp-fileserver", 100)
            .await
            .unwrap();
        assert_eq!(found, Some(newer));
    }

    #[tokio::test]
    async fn find_active_for_backend_skips_expired() {
        let store = InMemoryRefreshTokenStore::new();
        let mut expired = sample_record();
        expired.expires_at = 50;
        store.put(expired).await.unwrap();
        let found = store
            .find_active_for_backend("alice", "mcp-fileserver", 100)
            .await
            .unwrap();
        assert_eq!(found, None);
    }
}
