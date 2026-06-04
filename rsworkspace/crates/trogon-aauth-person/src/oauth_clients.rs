//! OAuth client registration records and an in-memory store.
//!
//! Per `GCP_TODO.md §3.6`: client registration is **record-based**,
//! not dynamic. Admins post `OAuthClientRecord`s via NATS (mirror of
//! `wif_admin`). This module ships the record + the trait + an
//! in-memory implementation. A JetStream-backed store can be added
//! later behind the same trait.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthClientLifecycleState {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthClientRecord {
    pub client_id: String,
    pub redirect_uris: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_scopes: Vec<String>,
    pub lifecycle_state: OAuthClientLifecycleState,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

impl OAuthClientRecord {
    pub fn allows_redirect(&self, uri: &str) -> bool {
        self.redirect_uris.iter().any(|u| u == uri)
    }

    pub fn allows_scope(&self, scope: &str) -> bool {
        if self.allowed_scopes.is_empty() {
            return true;
        }
        scope
            .split_whitespace()
            .all(|s| self.allowed_scopes.iter().any(|allowed| allowed == s))
    }

    pub fn is_enabled(&self) -> bool {
        self.lifecycle_state == OAuthClientLifecycleState::Enabled
    }
}

#[derive(Debug)]
pub enum OAuthClientStoreError {
    Backend(String),
}

impl std::fmt::Display for OAuthClientStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(message) => write!(f, "oauth client store: {message}"),
        }
    }
}

impl std::error::Error for OAuthClientStoreError {}

#[async_trait]
pub trait OAuthClientStore: Send + Sync {
    async fn put(&self, record: OAuthClientRecord) -> Result<(), OAuthClientStoreError>;
    async fn get(&self, client_id: &str) -> Result<Option<OAuthClientRecord>, OAuthClientStoreError>;
}

#[derive(Default)]
pub struct InMemoryOAuthClientStore {
    records: Mutex<HashMap<String, OAuthClientRecord>>,
}

impl InMemoryOAuthClientStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OAuthClientStore for InMemoryOAuthClientStore {
    async fn put(&self, record: OAuthClientRecord) -> Result<(), OAuthClientStoreError> {
        let mut guard = self.records.lock().expect("oauth client store lock");
        guard.insert(record.client_id.clone(), record);
        Ok(())
    }

    async fn get(&self, client_id: &str) -> Result<Option<OAuthClientRecord>, OAuthClientStoreError> {
        let guard = self.records.lock().expect("oauth client store lock");
        Ok(guard.get(client_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> OAuthClientRecord {
        OAuthClientRecord {
            client_id: "cursor-cli".into(),
            redirect_uris: vec!["http://127.0.0.1:8765/callback".into()],
            allowed_scopes: vec!["read:tools".into(), "call:tools".into()],
            lifecycle_state: OAuthClientLifecycleState::Enabled,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn in_memory_round_trip() {
        let store = InMemoryOAuthClientStore::new();
        store.put(sample()).await.expect("put");
        let got = store.get("cursor-cli").await.expect("get").expect("present");
        assert_eq!(got, sample());
    }

    #[tokio::test]
    async fn missing_client_returns_none() {
        let store = InMemoryOAuthClientStore::new();
        assert!(store.get("nope").await.expect("get").is_none());
    }

    #[test]
    fn redirect_uri_must_match_exactly() {
        let r = sample();
        assert!(r.allows_redirect("http://127.0.0.1:8765/callback"));
        assert!(!r.allows_redirect("http://127.0.0.1:8765/callback/extra"));
        assert!(!r.allows_redirect("https://attacker.example/callback"));
    }

    #[test]
    fn scope_must_be_subset_when_allowed_scopes_set() {
        let r = sample();
        assert!(r.allows_scope("read:tools"));
        assert!(r.allows_scope("read:tools call:tools"));
        assert!(!r.allows_scope("admin:tools"));
    }

    #[test]
    fn empty_allowed_scopes_admits_any_scope() {
        let mut r = sample();
        r.allowed_scopes.clear();
        assert!(r.allows_scope("anything goes"));
    }
}
