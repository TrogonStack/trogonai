use async_nats::jetstream::kv;
use bytes::Bytes;
use futures_util::StreamExt as _;
use std::future::Future;

use crate::types::{EvaluationResult, OutcomesError, Rubric};

// ── OutcomesStore trait ───────────────────────────────────────────────────────

pub trait OutcomesStore: Send + Sync + Clone + 'static {
    type PutError: std::error::Error + Send + Sync + 'static;
    type GetError: std::error::Error + Send + Sync + 'static;
    type DeleteError: std::error::Error + Send + Sync + 'static;
    type KeysError: std::error::Error + Send + Sync + 'static;

    fn put(
        &self,
        key: &str,
        value: Bytes,
    ) -> impl Future<Output = Result<u64, Self::PutError>> + Send;

    fn get(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::GetError>> + Send;

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), Self::DeleteError>> + Send;

    fn keys(&self) -> impl Future<Output = Result<Vec<String>, Self::KeysError>> + Send;
}

// ── Production implementation ─────────────────────────────────────────────────

#[derive(Debug)]
pub enum KvKeysError {
    Open(kv::HistoryError),
    Item(kv::WatcherError),
}

impl std::fmt::Display for KvKeysError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KvKeysError::Open(e) => write!(f, "failed to open keys stream: {e}"),
            KvKeysError::Item(e) => write!(f, "failed to iterate key: {e}"),
        }
    }
}

impl std::error::Error for KvKeysError {}

impl OutcomesStore for kv::Store {
    type PutError = kv::PutError;
    type GetError = kv::EntryError;
    type DeleteError = kv::DeleteError;
    type KeysError = KvKeysError;

    async fn put(&self, key: &str, value: Bytes) -> Result<u64, Self::PutError> {
        kv::Store::put(self, key, value).await
    }

    async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
        kv::Store::get(self, key).await
    }

    async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
        kv::Store::delete(self, key).await
    }

    async fn keys(&self) -> Result<Vec<String>, Self::KeysError> {
        let mut stream = kv::Store::keys(self).await.map_err(KvKeysError::Open)?;
        let mut keys = Vec::new();
        while let Some(k) = stream.next().await {
            keys.push(k.map_err(KvKeysError::Item)?);
        }
        Ok(keys)
    }
}

// ── RubricClient ──────────────────────────────────────────────────────────────

/// Typed read/write access for rubric definitions.
#[derive(Clone)]
pub struct RubricClient<S: OutcomesStore> {
    store: S,
}

impl<S: OutcomesStore> RubricClient<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub async fn put(&self, rubric: &Rubric) -> Result<(), OutcomesError> {
        let bytes = serde_json::to_vec(rubric)
            .map(Bytes::from)
            .map_err(|e| OutcomesError::Store(e.to_string()))?;
        self.store
            .put(&rubric.id, bytes)
            .await
            .map(|_| ())
            .map_err(|e| OutcomesError::Store(e.to_string()))
    }

    pub async fn get(&self, id: &str) -> Result<Option<Rubric>, OutcomesError> {
        match self.store.get(id).await {
            Ok(Some(b)) => serde_json::from_slice::<Rubric>(&b)
                .map(Some)
                .map_err(|e| OutcomesError::Store(e.to_string())),
            Ok(None) => Ok(None),
            Err(e) => Err(OutcomesError::Store(e.to_string())),
        }
    }

    pub async fn list(&self) -> Result<Vec<Rubric>, OutcomesError> {
        let keys = self
            .store
            .keys()
            .await
            .map_err(|e| OutcomesError::Store(e.to_string()))?;
        let mut rubrics = Vec::new();
        for key in keys {
            if let Some(r) = self.get(&key).await? {
                rubrics.push(r);
            }
        }
        Ok(rubrics)
    }

    pub async fn delete(&self, id: &str) -> Result<(), OutcomesError> {
        self.store
            .delete(id)
            .await
            .map_err(|e| OutcomesError::Store(e.to_string()))
    }
}

// ── ResultClient ──────────────────────────────────────────────────────────────

/// Key format for evaluation results: `{session_id}/{rubric_id}`.
pub fn result_key(session_id: &str, rubric_id: &str) -> String {
    format!("{session_id}.{rubric_id}")
}

/// Typed read/write access for evaluation results.
#[derive(Clone)]
pub struct ResultClient<S: OutcomesStore> {
    store: S,
}

impl<S: OutcomesStore> ResultClient<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub async fn put(&self, result: &EvaluationResult) -> Result<(), OutcomesError> {
        let key = result_key(&result.session_id, &result.rubric_id);
        let bytes = serde_json::to_vec(result)
            .map(Bytes::from)
            .map_err(|e| OutcomesError::Store(e.to_string()))?;
        self.store
            .put(&key, bytes)
            .await
            .map(|_| ())
            .map_err(|e| OutcomesError::Store(e.to_string()))
    }

    pub async fn get(
        &self,
        session_id: &str,
        rubric_id: &str,
    ) -> Result<Option<EvaluationResult>, OutcomesError> {
        let key = result_key(session_id, rubric_id);
        match self.store.get(&key).await {
            Ok(Some(b)) => serde_json::from_slice::<EvaluationResult>(&b)
                .map(Some)
                .map_err(|e| OutcomesError::Store(e.to_string())),
            Ok(None) => Ok(None),
            Err(e) => Err(OutcomesError::Store(e.to_string())),
        }
    }

    /// List all results for a given session (across all rubrics).
    pub async fn list_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<EvaluationResult>, OutcomesError> {
        let prefix = format!("{session_id}.");
        let keys = self
            .store
            .keys()
            .await
            .map_err(|e| OutcomesError::Store(e.to_string()))?;
        let mut results = Vec::new();
        for key in keys.into_iter().filter(|k| k.starts_with(&prefix)) {
            let parts: Vec<&str> = key.splitn(2, '.').collect();
            if parts.len() == 2 {
                if let Some(r) = self.get(parts[0], parts[1]).await? {
                    results.push(r);
                }
            }
        }
        Ok(results)
    }
}

// ── Mock implementation ───────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct MockOutcomesStore {
        data: Arc<Mutex<HashMap<String, Bytes>>>,
    }

    impl MockOutcomesStore {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl OutcomesStore for MockOutcomesStore {
        type PutError = std::convert::Infallible;
        type GetError = std::convert::Infallible;
        type DeleteError = std::convert::Infallible;
        type KeysError = std::convert::Infallible;

        async fn put(&self, key: &str, value: Bytes) -> Result<u64, Self::PutError> {
            self.data.lock().unwrap().insert(key.to_string(), value);
            Ok(0)
        }

        async fn get(&self, key: &str) -> Result<Option<Bytes>, Self::GetError> {
            Ok(self.data.lock().unwrap().get(key).cloned())
        }

        async fn delete(&self, key: &str) -> Result<(), Self::DeleteError> {
            self.data.lock().unwrap().remove(key);
            Ok(())
        }

        async fn keys(&self) -> Result<Vec<String>, Self::KeysError> {
            Ok(self.data.lock().unwrap().keys().cloned().collect())
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Criterion, CriterionScore, EvaluationResult};
    use mock::MockOutcomesStore;

    fn rubric(id: &str) -> Rubric {
        Rubric::new(
            id,
            "Test",
            "desc",
            vec![Criterion {
                name: "quality".into(),
                description: "Is the output high quality?".into(),
                weight: 1.0,
            }],
        )
    }

    fn result(session_id: &str, rubric_id: &str, score: f32) -> EvaluationResult {
        let rubric = rubric(rubric_id);
        let scores = vec![CriterionScore {
            criterion: "quality".into(),
            score,
            reasoning: "good".into(),
        }];
        EvaluationResult::compute(&rubric, session_id, "pr", "repo/1", scores, "looks good")
    }

    // ── RubricClient ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rubric_client_put_and_get_round_trips() {
        let client = RubricClient::new(MockOutcomesStore::new());
        let r = rubric("r1");
        client.put(&r).await.unwrap();
        let loaded = client.get("r1").await.unwrap().unwrap();
        assert_eq!(loaded.id, "r1");
        assert_eq!(loaded.name, "Test");
    }

    #[tokio::test]
    async fn rubric_client_get_returns_none_for_missing() {
        let client = RubricClient::new(MockOutcomesStore::new());
        assert!(client.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rubric_client_list_returns_all() {
        let client = RubricClient::new(MockOutcomesStore::new());
        client.put(&rubric("r1")).await.unwrap();
        client.put(&rubric("r2")).await.unwrap();
        let list = client.list().await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn rubric_client_delete_removes_rubric() {
        let client = RubricClient::new(MockOutcomesStore::new());
        client.put(&rubric("r1")).await.unwrap();
        client.delete("r1").await.unwrap();
        assert!(client.get("r1").await.unwrap().is_none());
    }

    // ── ResultClient ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn result_client_put_and_get_round_trips() {
        let client = ResultClient::new(MockOutcomesStore::new());
        let r = result("sess-1", "r1", 0.9);
        client.put(&r).await.unwrap();
        let loaded = client.get("sess-1", "r1").await.unwrap().unwrap();
        assert_eq!(loaded.session_id, "sess-1");
        assert_eq!(loaded.rubric_id, "r1");
        assert!((loaded.overall_score - 0.9).abs() < 1e-5);
    }

    #[tokio::test]
    async fn result_client_list_for_session_returns_all_rubrics() {
        let store = MockOutcomesStore::new();
        let client = ResultClient::new(store);
        client.put(&result("sess-1", "r1", 0.8)).await.unwrap();
        client.put(&result("sess-1", "r2", 0.6)).await.unwrap();
        client.put(&result("sess-2", "r1", 0.5)).await.unwrap();
        let results = client.list_for_session("sess-1").await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.session_id == "sess-1"));
    }

    #[tokio::test]
    async fn list_for_session_does_not_bleed_into_prefix_sharing_sessions() {
        // "sess-1" is a prefix of "sess-10" — list_for_session("sess-1") must
        // not return results belonging to "sess-10".
        let store = MockOutcomesStore::new();
        let client = ResultClient::new(store);
        client.put(&result("sess-1", "r1", 0.9)).await.unwrap();
        client.put(&result("sess-10", "r1", 0.5)).await.unwrap();

        let results = client.list_for_session("sess-1").await.unwrap();
        assert_eq!(results.len(), 1, "should not include sess-10 results");
        assert_eq!(results[0].session_id, "sess-1");
    }

    #[tokio::test]
    async fn result_key_format_is_session_dot_rubric() {
        assert_eq!(result_key("sess-abc", "rubric-1"), "sess-abc.rubric-1");
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_result() {
        let client = ResultClient::new(MockOutcomesStore::new());
        assert!(client.get("sess-missing", "r1").await.unwrap().is_none());
    }
}
