//! Durable promise store — NATS KV-backed checkpoint mechanism for agent runs.
//!
//! Before executing any step, check if a result already exists in KV. If it
//! does, return the cached result without re-executing. If it does not, execute,
//! store the result, then continue. On restart, the run resumes from the last
//! checkpoint rather than from scratch.
//!
//! # Buckets
//! - [`AGENT_PROMISES_BUCKET`]: full run state, checkpointed after each LLM turn.
//! - [`AGENT_TOOL_RESULTS_BUCKET`]: cached result per tool call.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::agent_loop::Message;

/// NATS KV bucket for per-run checkpoints.
pub const AGENT_PROMISES_BUCKET: &str = "AGENT_PROMISES";
/// NATS KV bucket for cached tool call results.
pub const AGENT_TOOL_RESULTS_BUCKET: &str = "AGENT_TOOL_RESULTS";

/// TTL for both buckets — long enough for the longest possible run.
const PROMISE_TTL: Duration = Duration::from_secs(24 * 3600);

// ── PromiseStatus ─────────────────────────────────────────────────────────────

/// Lifecycle status of an [`AgentPromise`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromiseStatus {
    /// The run is in-progress.
    Running,
    /// The run completed successfully.
    Resolved,
    /// The run failed with a transient error (e.g. HTTP timeout).
    ///
    /// On NATS redelivery the run is retried from its checkpoint.
    Failed,
    /// The run failed with a deterministic error that retrying cannot fix
    /// (e.g. `max_tokens`, `MaxIterationsReached`, unknown stop_reason).
    ///
    /// Neither startup recovery nor NATS redelivery will re-run this promise.
    /// Treated the same as `Resolved` in `prepare_agent_with_promise` and
    /// `AgentLoop::run`.
    PermanentFailed,
}

// ── AgentPromise ──────────────────────────────────────────────────────────────

/// A durable record of an in-progress or completed agent run.
///
/// Stored in [`AGENT_PROMISES_BUCKET`] with key `{tenant_id}.{promise_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPromise {
    /// Unique run identifier — `{tenant_id}.{nats_stream_seq}` for single-handler
    /// runs, or `{tenant_id}.{nats_stream_seq}.{automation_id}` for automation runs.
    pub id: String,
    /// Tenant that owns this run.
    pub tenant_id: String,
    /// Automation ID for the run (empty string for built-in handler runs).
    pub automation_id: String,
    /// Lifecycle status.
    pub status: PromiseStatus,
    /// Conversation history checkpoint — updated after each LLM turn.
    pub messages: Vec<Message>,
    /// How many LLM iterations have completed.
    pub iteration: u32,
    /// Which process claimed this run (hostname + PID).
    pub worker_id: String,
    /// When this worker claimed the promise (Unix seconds).
    pub claimed_at: u64,
    /// Original trigger payload (NATS message body).
    pub trigger: serde_json::Value,
    /// Original NATS message subject — used to re-dispatch on startup recovery.
    pub nats_subject: String,
    /// System prompt captured at the first checkpoint of this run.
    ///
    /// Stored so that crash recovery resumes with the identical LLM context
    /// rather than a potentially-changed `memory.md` fetched at restart time.
    /// `None` for promises written before this field was introduced —
    /// `#[serde(default)]` deserializes them gracefully.
    #[serde(default)]
    pub system_prompt: Option<String>,
}

// ── PromiseStoreError ─────────────────────────────────────────────────────────

/// An error from the promise store.
#[derive(Debug)]
pub struct PromiseStoreError(pub String);

impl std::fmt::Display for PromiseStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PromiseStoreError {}

// ── key helpers ───────────────────────────────────────────────────────────────

fn promise_key(tenant_id: &str, promise_id: &str) -> String {
    format!("{tenant_id}.{promise_id}")
}

/// Build a NATS KV key for a cached tool result.
///
/// `cache_key` is a hex SHA-256 digest computed by the caller from
/// `(tool_name, canonical_json(input))` — see `agent_loop::tool_cache_key`.
/// Hex strings never contain dots, so no escaping is needed.
fn tool_result_key(tenant_id: &str, promise_id: &str, cache_key: &str) -> String {
    format!("{tenant_id}.{promise_id}.{cache_key}")
}

// ── PromiseStore ──────────────────────────────────────────────────────────────

/// NATS KV-backed store for agent promises and tool-result caches.
#[derive(Clone)]
pub struct PromiseStore {
    promises: kv::Store,
    tool_results: kv::Store,
}

impl PromiseStore {
    /// Open (or create) the `AGENT_PROMISES` and `AGENT_TOOL_RESULTS` KV buckets.
    pub async fn open(js: &jetstream::Context) -> Result<Self, PromiseStoreError> {
        // OS crash durability note: `async-nats` 0.47.0 does not expose the
        // `sync_always` field on `kv::Config` or the underlying `stream::Config`,
        // so we cannot set it programmatically here. The NATS server must be
        // configured at the cluster level:
        //
        //   jetstream {
        //       sync_always: true
        //   }
        //
        // Without this, NATS fsyncs every ~2 minutes by default. A write
        // acknowledged by NATS but not yet fsynced can be lost on a kernel
        // panic or power failure (process crashes are not affected). Setting
        // `sync_always: true` server-side closes this gap.
        //
        // Alternative: run a 3-node NATS cluster — quorum writes require
        // acknowledgement from a majority of replicas, giving WAL-equivalent
        // durability without the per-write fsync overhead.
        let promises = js
            .create_or_update_key_value(kv::Config {
                bucket: AGENT_PROMISES_BUCKET.to_string(),
                history: 1,
                max_age: PROMISE_TTL,
                ..Default::default()
            })
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))?;

        let tool_results = js
            .create_or_update_key_value(kv::Config {
                bucket: AGENT_TOOL_RESULTS_BUCKET.to_string(),
                history: 1,
                max_age: PROMISE_TTL,
                ..Default::default()
            })
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))?;

        Ok(Self {
            promises,
            tool_results,
        })
    }

    /// Store a promise (create or overwrite), returning the new KV revision.
    pub async fn put_promise(&self, promise: &AgentPromise) -> Result<u64, PromiseStoreError> {
        let key = promise_key(&promise.tenant_id, &promise.id);
        let bytes = serde_json::to_vec(promise).map_err(|e| PromiseStoreError(e.to_string()))?;
        self.promises
            .put(&key, Bytes::from(bytes))
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))
    }

    /// Fetch a promise and its current KV revision.
    ///
    /// The revision is used as the CAS token for [`update_promise`].
    pub async fn get_promise(
        &self,
        tenant_id: &str,
        promise_id: &str,
    ) -> Result<Option<(AgentPromise, u64)>, PromiseStoreError> {
        let key = promise_key(tenant_id, promise_id);
        match self
            .promises
            .entry(&key)
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))?
        {
            None => Ok(None),
            Some(entry) => {
                // `entry()` returns tombstones (Delete / Purge operations) as
                // well as live entries. Treat them as "not found" rather than
                // trying to deserialize empty bytes, which would produce a JSON
                // parse error and fail the entire `list_running` scan.
                if entry.operation != kv::Operation::Put {
                    return Ok(None);
                }
                let p = serde_json::from_slice::<AgentPromise>(&entry.value)
                    .map_err(|e| PromiseStoreError(e.to_string()))?;
                Ok(Some((p, entry.revision)))
            }
        }
    }

    /// Compare-and-swap update a promise using its last-known revision.
    ///
    /// Returns the new revision on success. Returns an error if another writer
    /// updated the promise since the last [`get_promise`] call — the caller
    /// should reload and retry if needed.
    pub async fn update_promise(
        &self,
        tenant_id: &str,
        promise_id: &str,
        promise: &AgentPromise,
        revision: u64,
    ) -> Result<u64, PromiseStoreError> {
        let key = promise_key(tenant_id, promise_id);
        let bytes = serde_json::to_vec(promise).map_err(|e| PromiseStoreError(e.to_string()))?;
        self.promises
            .update(&key, Bytes::from(bytes), revision)
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))
    }

    /// Cache a tool result so it can be replayed on restart without re-executing the tool.
    ///
    /// `cache_key` must be the SHA-256 hex digest of `(tool_name, input)` — see
    /// `agent_loop::tool_cache_key`. Using a content-based key (rather than
    /// Anthropic's ephemeral `tool_use_id`) means the cached result survives a
    /// process restart: the LLM re-generates a fresh `tool_use_id` on recovery,
    /// but the name and input are identical, so the hash matches and the tool
    /// is not re-executed.
    pub async fn put_tool_result(
        &self,
        tenant_id: &str,
        promise_id: &str,
        cache_key: &str,
        result: &str,
    ) -> Result<(), PromiseStoreError> {
        let key = tool_result_key(tenant_id, promise_id, cache_key);
        self.tool_results
            .put(&key, Bytes::from(result.as_bytes().to_vec()))
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))?;
        Ok(())
    }

    /// Return all promises for `tenant_id` that are currently in [`PromiseStatus::Running`].
    async fn list_running_inner(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<AgentPromise>, PromiseStoreError> {
        use futures_util::TryStreamExt;
        let prefix = format!("{tenant_id}.");
        let mut keys = self
            .promises
            .keys()
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))?;
        let mut result = Vec::new();
        while let Some(key) = keys
            .try_next()
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))?
        {
            if !key.starts_with(&prefix) {
                continue;
            }
            let promise_id = &key[prefix.len()..];
            if let Some((p, _)) = self.get_promise(tenant_id, promise_id).await?
                && p.status == PromiseStatus::Running
            {
                result.push(p);
            }
        }
        Ok(result)
    }

    /// Return a cached tool result if it exists.
    ///
    /// `cache_key` must be the same SHA-256 hex digest passed to [`put_tool_result`].
    pub async fn get_tool_result(
        &self,
        tenant_id: &str,
        promise_id: &str,
        cache_key: &str,
    ) -> Result<Option<String>, PromiseStoreError> {
        let key = tool_result_key(tenant_id, promise_id, cache_key);
        match self
            .tool_results
            .get(&key)
            .await
            .map_err(|e| PromiseStoreError(e.to_string()))?
        {
            None => Ok(None),
            Some(bytes) => {
                let s = String::from_utf8(bytes.to_vec())
                    .map_err(|e| PromiseStoreError(e.to_string()))?;
                Ok(Some(s))
            }
        }
    }
}

// ── PromiseRepository trait ──────────────────────────────────────────────────

/// Abstraction over a promise store.
///
/// Implementing this trait allows replacing the real NATS KV backend with a
/// lightweight in-memory fake in unit tests.
///
/// The trait intentionally omits `Clone` from its supertraits so that it can be
/// used as `Arc<dyn PromiseRepository>`. Concrete implementations (`PromiseStore`,
/// `MockPromiseStore`) derive `Clone` independently.
/// Return type for `get_promise` — the promise paired with its KV revision.
pub type PromiseEntry = (AgentPromise, u64);

pub trait PromiseRepository: Send + Sync + 'static {
    fn get_promise<'a>(
        &'a self,
        tenant_id: &'a str,
        promise_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PromiseEntry>, PromiseStoreError>> + Send + 'a>>;

    fn put_promise<'a>(
        &'a self,
        promise: &'a AgentPromise,
    ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>>;

    fn update_promise<'a>(
        &'a self,
        tenant_id: &'a str,
        promise_id: &'a str,
        promise: &'a AgentPromise,
        revision: u64,
    ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>>;

    fn get_tool_result<'a>(
        &'a self,
        tenant_id: &'a str,
        promise_id: &'a str,
        cache_key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, PromiseStoreError>> + Send + 'a>>;

    fn put_tool_result<'a>(
        &'a self,
        tenant_id: &'a str,
        promise_id: &'a str,
        cache_key: &'a str,
        result: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PromiseStoreError>> + Send + 'a>>;

    fn list_running<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AgentPromise>, PromiseStoreError>> + Send + 'a>>;
}

impl PromiseRepository for PromiseStore {
    fn get_promise<'a>(
        &'a self,
        tenant_id: &'a str,
        promise_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PromiseEntry>, PromiseStoreError>> + Send + 'a>>
    {
        Box::pin(async move { self.get_promise(tenant_id, promise_id).await })
    }

    fn put_promise<'a>(
        &'a self,
        promise: &'a AgentPromise,
    ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>> {
        Box::pin(async move { self.put_promise(promise).await })
    }

    fn update_promise<'a>(
        &'a self,
        tenant_id: &'a str,
        promise_id: &'a str,
        promise: &'a AgentPromise,
        revision: u64,
    ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.update_promise(tenant_id, promise_id, promise, revision)
                .await
        })
    }

    fn get_tool_result<'a>(
        &'a self,
        tenant_id: &'a str,
        promise_id: &'a str,
        cache_key: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, PromiseStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.get_tool_result(tenant_id, promise_id, cache_key)
                .await
        })
    }

    fn put_tool_result<'a>(
        &'a self,
        tenant_id: &'a str,
        promise_id: &'a str,
        cache_key: &'a str,
        result: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), PromiseStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.put_tool_result(tenant_id, promise_id, cache_key, result)
                .await
        })
    }

    fn list_running<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AgentPromise>, PromiseStoreError>> + Send + 'a>>
    {
        Box::pin(async move { self.list_running_inner(tenant_id).await })
    }
}

// ── In-memory mock (test-only) ────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// HashMap-backed in-memory promise store for unit tests.
    #[derive(Clone, Default)]
    pub struct MockPromiseStore {
        promises: Arc<Mutex<HashMap<String, (AgentPromise, u64)>>>,
        tool_results: Arc<Mutex<HashMap<String, String>>>,
    }

    impl MockPromiseStore {
        pub fn new() -> Self {
            Self::default()
        }

        /// Pre-populate with a promise at revision 1.
        pub fn insert_promise(&self, promise: AgentPromise) {
            let key = format!("{}.{}", promise.tenant_id, promise.id);
            self.promises.lock().unwrap().insert(key, (promise, 1));
        }

        /// Snapshot current promises.
        pub fn snapshot_promises(&self) -> HashMap<String, (AgentPromise, u64)> {
            self.promises.lock().unwrap().clone()
        }
    }

    impl PromiseRepository for MockPromiseStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<
            Box<dyn Future<Output = Result<Option<PromiseEntry>, PromiseStoreError>> + Send + 'a>,
        > {
            let key = format!("{tenant_id}.{promise_id}");
            let data = Arc::clone(&self.promises);
            Box::pin(async move { Ok(data.lock().unwrap().get(&key).cloned()) })
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>> {
            let data = Arc::clone(&self.promises);
            let promise = promise.clone();
            Box::pin(async move {
                let key = format!("{}.{}", promise.tenant_id, promise.id);
                let mut guard = data.lock().unwrap();
                let rev = guard.get(&key).map(|(_, r)| r + 1).unwrap_or(1);
                guard.insert(key, (promise, rev));
                Ok(rev)
            })
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>> {
            let key = format!("{tenant_id}.{promise_id}");
            let data = Arc::clone(&self.promises);
            let promise = promise.clone();
            Box::pin(async move {
                let mut guard = data.lock().unwrap();
                match guard.get(&key) {
                    Some((_, current_rev)) if *current_rev != revision => Err(PromiseStoreError(
                        format!("CAS mismatch: expected revision {revision}, got {current_rev}"),
                    )),
                    _ => {
                        let new_rev = revision + 1;
                        guard.insert(key, (promise, new_rev));
                        Ok(new_rev)
                    }
                }
            })
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, PromiseStoreError>> + Send + 'a>>
        {
            let key = tool_result_key(tenant_id, promise_id, cache_key);
            let data = Arc::clone(&self.tool_results);
            Box::pin(async move { Ok(data.lock().unwrap().get(&key).cloned()) })
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), PromiseStoreError>> + Send + 'a>> {
            let key = tool_result_key(tenant_id, promise_id, cache_key);
            let data = Arc::clone(&self.tool_results);
            let result = result.to_string();
            Box::pin(async move {
                data.lock().unwrap().insert(key, result);
                Ok(())
            })
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<AgentPromise>, PromiseStoreError>> + Send + 'a>>
        {
            let data = Arc::clone(&self.promises);
            let prefix = format!("{tenant_id}.");
            Box::pin(async move {
                let guard = data.lock().unwrap();
                let result = guard
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .filter(|(_, (p, _))| p.status == PromiseStatus::Running)
                    .map(|(_, (p, _))| p.clone())
                    .collect();
                Ok(result)
            })
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mock::MockPromiseStore;

    fn sample_promise(id: &str) -> AgentPromise {
        AgentPromise {
            id: id.to_string(),
            tenant_id: "acme".to_string(),
            automation_id: "auto-1".to_string(),
            status: PromiseStatus::Running,
            messages: vec![],
            iteration: 0,
            worker_id: "worker-1".to_string(),
            claimed_at: 1_700_000_000,
            trigger: serde_json::json!({"action": "opened"}),
            nats_subject: "github.pull_request".to_string(),
            system_prompt: None,
        }
    }

    #[test]
    fn promise_key_format() {
        assert_eq!(promise_key("acme", "abc123"), "acme.abc123");
    }

    #[test]
    fn tool_result_key_format() {
        // cache_key is a hex SHA-256 digest — no dots, no escaping needed.
        let key = tool_result_key("acme", "abc123", "deadbeef");
        assert_eq!(key, "acme.abc123.deadbeef");
    }

    #[test]
    fn promise_status_round_trips_json() {
        for status in [
            PromiseStatus::Running,
            PromiseStatus::Resolved,
            PromiseStatus::Failed,
            PromiseStatus::PermanentFailed,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: PromiseStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn agent_promise_round_trips_json() {
        let p = sample_promise("run-1");
        let json = serde_json::to_string(&p).unwrap();
        let back: AgentPromise = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "run-1");
        assert_eq!(back.tenant_id, "acme");
    }

    #[tokio::test]
    async fn mock_put_and_get_promise() {
        let store = MockPromiseStore::new();
        let p = sample_promise("run-1");
        let rev = store.put_promise(&p).await.unwrap();
        assert_eq!(rev, 1);

        let (fetched, fetched_rev) = store
            .get_promise("acme", "run-1")
            .await
            .unwrap()
            .expect("should exist");
        assert_eq!(fetched.id, "run-1");
        assert_eq!(fetched_rev, 1);
    }

    #[tokio::test]
    async fn mock_update_promise_cas() {
        let store = MockPromiseStore::new();
        let p = sample_promise("run-1");
        let rev = store.put_promise(&p).await.unwrap();

        let mut updated = p.clone();
        updated.iteration = 1;
        let new_rev = store
            .update_promise("acme", "run-1", &updated, rev)
            .await
            .unwrap();
        assert_eq!(new_rev, 2);

        // Stale revision should fail.
        let err = store
            .update_promise("acme", "run-1", &updated, rev)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("CAS mismatch"));
    }

    #[tokio::test]
    async fn mock_tool_result_round_trip() {
        let store = MockPromiseStore::new();
        // cache_key is a SHA-256 hex digest in production; use an opaque string here.
        let cache_key = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        store
            .put_tool_result("acme", "run-1", cache_key, "result text")
            .await
            .unwrap();
        let cached = store
            .get_tool_result("acme", "run-1", cache_key)
            .await
            .unwrap();
        assert_eq!(cached, Some("result text".to_string()));
    }

    #[tokio::test]
    async fn mock_get_missing_promise_returns_none() {
        let store = MockPromiseStore::new();
        let result = store.get_promise("acme", "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn mock_get_missing_tool_result_returns_none() {
        let store = MockPromiseStore::new();
        let result = store
            .get_tool_result("acme", "run-1", "0000000000000000000000000000000000000000000000000000000000000000")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn promise_store_error_display() {
        let e = PromiseStoreError("bucket gone".to_string());
        assert!(e.to_string().contains("bucket gone"));
    }
}
