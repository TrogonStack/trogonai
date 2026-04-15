use async_nats::jetstream::{
    self,
    context::{CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind},
    ErrorCode,
    kv,
    stream,
};
use bytes::Bytes;
use std::future::Future;

use crate::error::SaveError;

// ── Constants ─────────────────────────────────────────────────────────────────

pub const BUCKET_NAME: &str = "ACTOR_STATE";

/// Maximum times the runtime retries after an optimistic concurrency conflict
/// before giving up and returning [`crate::error::ActorError::RetryLimitExceeded`].
pub const MAX_OCC_RETRIES: u32 = 10;

// ── Key helpers ───────────────────────────────────────────────────────────────

/// Sanitize an arbitrary entity key into a valid NATS KV key token.
///
/// Uses the same rules as `trogon-transcript`'s `sanitize_key`: `/` → `.`,
/// non-alphanumeric chars (except `-`, `_`, `.`) → `-`.
pub fn sanitize_key_part(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            '/' => '.',
            c if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' => c,
            _ => '-',
        })
        .collect()
}

/// The NATS KV key used to store state for a given `(actor_type, entity_key)`.
///
/// Format: `{actor_type}.{sanitized_entity_key}`
pub fn state_kv_key(actor_type: &str, entity_key: &str) -> String {
    format!("{}.{}", actor_type, sanitize_key_part(entity_key))
}

// ── StateEntry ────────────────────────────────────────────────────────────────

/// The value loaded from the KV store: the serialized state bytes plus the
/// revision number used for the next optimistic-concurrency save.
pub struct StateEntry {
    pub value: Bytes,
    pub revision: u64,
}

// ── StateStore trait ──────────────────────────────────────────────────────────

/// Abstraction over the NATS KV operations needed for actor state persistence.
///
/// The real implementation is `kv::Store`. The mock is `MockStateStore`.
pub trait StateStore: Send + Sync + Clone + 'static {
    type LoadError: std::error::Error + Send + Sync + 'static;
    type DeleteError: std::error::Error + Send + Sync + 'static;

    /// Load the current state for `key`. Returns `None` if this is the first
    /// event for the entity.
    fn load(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<StateEntry>, Self::LoadError>> + Send;

    /// Persist `value` for `key`.
    ///
    /// - `revision = None`: create a new entry (entity has no prior state).
    /// - `revision = Some(r)`: update the entry only if its current revision
    ///   matches `r`. Returns [`SaveError::Conflict`] if stale.
    fn save(
        &self,
        key: &str,
        value: Bytes,
        revision: Option<u64>,
    ) -> impl Future<Output = Result<u64, SaveError>> + Send;

    /// Delete the state for `key` (called by `on_destroy`).
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Self::DeleteError>> + Send;
}

// ── Production implementation (kv::Store) ────────────────────────────────────

impl StateStore for kv::Store {
    type LoadError = kv::EntryError;
    type DeleteError = kv::DeleteError;

    async fn load(&self, key: &str) -> Result<Option<StateEntry>, Self::LoadError> {
        match kv::Store::entry(self, key).await? {
            Some(entry) if entry.operation == kv::Operation::Put => Ok(Some(StateEntry {
                value: entry.value,
                revision: entry.revision,
            })),
            _ => Ok(None),
        }
    }

    async fn save(&self, key: &str, value: Bytes, revision: Option<u64>) -> Result<u64, SaveError> {
        match revision {
            None => {
                // First event — create new entry
                kv::Store::create(self, key, value)
                    .await
                    .map_err(|e| {
                        if is_create_conflict(&e) {
                            SaveError::Conflict
                        } else {
                            SaveError::Other(Box::new(e))
                        }
                    })
            }
            Some(rev) => {
                // Subsequent event — update with revision check
                kv::Store::update(self, key, value, rev)
                    .await
                    .map_err(|e| {
                        if is_update_conflict(&e) {
                            SaveError::Conflict
                        } else {
                            SaveError::Other(Box::new(e))
                        }
                    })
            }
        }
    }

    async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
        kv::Store::delete(self, key).await
    }
}

/// A `create` fails with "already exists" when two events race on a brand-new entity.
fn is_create_conflict(error: &kv::CreateError) -> bool {
    // async-nats surfaces this as a wrong-last-sequence JetStream error.
    let msg = error.to_string().to_lowercase();
    msg.contains("wrong last") || msg.contains("already exists") || msg.contains("expected sequence")
}

/// An `update` fails with "wrong last sequence" when the revision we read is stale.
fn is_update_conflict(error: &kv::UpdateError) -> bool {
    let msg = error.to_string().to_lowercase();
    msg.contains("wrong last") || msg.contains("expected sequence")
}

// ── Bucket provisioning ───────────────────────────────────────────────────────

/// Create or open the `ACTOR_STATE` KV bucket. Idempotent.
pub async fn provision_state(js: &jetstream::Context) -> Result<kv::Store, String> {
    match js
        .create_key_value(kv::Config {
            bucket: BUCKET_NAME.to_string(),
            // Only the latest revision per entity is needed.
            history: 1,
            // State is long-lived — no TTL. Entries are deleted by on_destroy.
            max_age: std::time::Duration::ZERO,
            storage: stream::StorageType::File,
            ..Default::default()
        })
        .await
    {
        Ok(store) => Ok(store),
        Err(e) if is_bucket_already_exists(&e) => js
            .get_key_value(BUCKET_NAME)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn is_bucket_already_exists(error: &CreateKeyValueError) -> bool {
    error.kind() == CreateKeyValueErrorKind::BucketCreate
        && std::error::Error::source(error)
            .and_then(|s| s.downcast_ref::<CreateStreamError>())
            .is_some_and(|s| {
                matches!(
                    s.kind(),
                    CreateStreamErrorKind::JetStream(ref j)
                        if j.error_code() == ErrorCode::STREAM_NAME_EXIST
                )
            })
}

// ── Mock implementation ───────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// Infallible error type for MockStateStore load and delete operations.
    #[derive(Debug)]
    pub struct MockLoadError(pub String);

    impl std::fmt::Display for MockLoadError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for MockLoadError {}

    #[derive(Clone, Default)]
    pub struct MockStateStore {
        data: Arc<Mutex<HashMap<String, (Bytes, u64)>>>,
        /// If set to `Some(n)`, the next `n` saves will return `Conflict`.
        conflict_count: Arc<Mutex<u32>>,
        /// If true, the next save will return `SaveError::Other`.
        next_save_other_error: Arc<Mutex<bool>>,
        /// If true, the next load will return an error.
        next_load_error: Arc<Mutex<bool>>,
    }

    impl MockStateStore {
        pub fn new() -> Self {
            Self::default()
        }

        /// Cause the next `n` calls to `save` to return `SaveError::Conflict`.
        pub fn inject_conflicts(&self, n: u32) {
            *self.conflict_count.lock().unwrap() = n;
        }

        /// Cause the next `save` to return `SaveError::Other`.
        pub fn inject_save_error(&self) {
            *self.next_save_other_error.lock().unwrap() = true;
        }

        /// Cause the next `load` to return an error.
        pub fn inject_load_error(&self) {
            *self.next_load_error.lock().unwrap() = true;
        }

        pub fn snapshot(&self) -> HashMap<String, Bytes> {
            self.data
                .lock()
                .unwrap()
                .iter()
                .map(|(k, (v, _))| (k.clone(), v.clone()))
                .collect()
        }
    }

    impl StateStore for MockStateStore {
        type LoadError = MockLoadError;
        type DeleteError = std::convert::Infallible;

        async fn load(&self, key: &str) -> Result<Option<StateEntry>, Self::LoadError> {
            let mut load_err = self.next_load_error.lock().unwrap();
            if *load_err {
                *load_err = false;
                return Err(MockLoadError("injected load error".to_string()));
            }
            drop(load_err);
            Ok(self
                .data
                .lock()
                .unwrap()
                .get(key)
                .map(|(v, rev)| StateEntry {
                    value: v.clone(),
                    revision: *rev,
                }))
        }

        async fn save(&self, key: &str, value: Bytes, _revision: Option<u64>) -> Result<u64, SaveError> {
            let mut save_err = self.next_save_other_error.lock().unwrap();
            if *save_err {
                *save_err = false;
                return Err(SaveError::Other(Box::new(MockLoadError("injected save error".to_string()))));
            }
            drop(save_err);

            let mut count = self.conflict_count.lock().unwrap();
            if *count > 0 {
                *count -= 1;
                return Err(SaveError::Conflict);
            }
            drop(count);

            let mut data = self.data.lock().unwrap();
            let next_rev = data
                .get(key)
                .map(|(_, r)| r + 1)
                .unwrap_or(1);
            data.insert(key.to_string(), (value, next_rev));
            Ok(next_rev)
        }

        async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
            self.data.lock().unwrap().remove(key);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock::MockStateStore;

    #[test]
    fn state_kv_key_sanitizes_slashes() {
        assert_eq!(state_kv_key("pr", "owner/repo/456"), "pr.owner.repo.456");
    }

    #[test]
    fn state_kv_key_sanitizes_special_chars() {
        assert_eq!(state_kv_key("incident", "inc:2024"), "incident.inc-2024");
    }

    #[tokio::test]
    async fn mock_load_returns_none_when_empty() {
        let store = MockStateStore::new();
        assert!(store.load("pr.owner.repo.1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mock_save_and_load_roundtrip() {
        let store = MockStateStore::new();
        let value = Bytes::from_static(b"hello");
        store.save("key", value.clone(), None).await.unwrap();
        let entry = store.load("key").await.unwrap().unwrap();
        assert_eq!(entry.value, value);
        assert_eq!(entry.revision, 1);
    }

    #[tokio::test]
    async fn mock_conflict_injection() {
        let store = MockStateStore::new();
        store.inject_conflicts(2);
        assert!(matches!(
            store.save("key", Bytes::new(), None).await,
            Err(SaveError::Conflict)
        ));
        assert!(matches!(
            store.save("key", Bytes::new(), None).await,
            Err(SaveError::Conflict)
        ));
        // Third save succeeds
        assert!(store.save("key", Bytes::new(), None).await.is_ok());
    }

    #[tokio::test]
    async fn mock_delete_removes_entry() {
        let store = MockStateStore::new();
        store.save("k", Bytes::from_static(b"v"), None).await.unwrap();
        store.delete("k").await.unwrap();
        assert!(store.load("k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mock_snapshot_reflects_saved_state() {
        let store = MockStateStore::new();
        store.save("a", Bytes::from_static(b"1"), None).await.unwrap();
        store.save("b", Bytes::from_static(b"2"), None).await.unwrap();
        let snap = store.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap["a"], Bytes::from_static(b"1"));
        assert_eq!(snap["b"], Bytes::from_static(b"2"));
    }

    #[tokio::test]
    async fn mock_inject_save_error_returns_other() {
        let store = MockStateStore::new();
        store.inject_save_error();
        let result = store.save("k", Bytes::new(), None).await;
        assert!(matches!(result, Err(SaveError::Other(_))));
        // Next save succeeds.
        assert!(store.save("k", Bytes::new(), None).await.is_ok());
    }

    #[tokio::test]
    async fn mock_inject_load_error_returns_err() {
        let store = MockStateStore::new();
        store.inject_load_error();
        assert!(store.load("k").await.is_err());
        // Next load succeeds (returns None).
        assert!(store.load("k").await.unwrap().is_none());
    }
}
