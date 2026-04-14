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
    /// Unique run identifier — `{subject_slug}.{nats_stream_seq}` for single-handler
    /// runs, or `{subject_slug}.{nats_stream_seq}.{automation_id}` for automation runs.
    /// `subject_slug` is the NATS subject with dots replaced by underscores and
    /// lowercased (e.g. `github_pull_request`, `linear_issue`), ensuring uniqueness
    /// even when separate JetStream streams share the same sequence counter.
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
    ///
    /// ## Performance note: N+1 KV reads
    ///
    /// This method does one `keys()` scan followed by one `get_promise` call per
    /// matching key. The `async-nats` KV API does not expose a bulk-read primitive
    /// (e.g. `values()` or `entries()`), so this N+1 pattern is the best available
    /// without dropping to the lower-level JetStream stream API.
    ///
    /// This is acceptable because `list_running` is only called once at startup.
    /// The bucket is bounded by the 24-hour TTL, so the total number of entries
    /// is proportional to one day's worth of trigger events — not unbounded.
    /// If startup latency from this scan becomes observable, consider switching to
    /// the raw JetStream direct-get API or maintaining a secondary `Running` index
    /// in a separate KV key.
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

    /// Wraps [`MockPromiseStore`] and injects exactly one CAS conflict on the
    /// first [`PromiseRepository::update_promise`] call.
    ///
    /// The inner store is not modified on the failing call, so a subsequent
    /// `get_promise` still returns the original revision — exactly what happens
    /// in production when another worker writes between our get and our write.
    ///
    /// Subsequent `update_promise` calls are delegated to the inner store and
    /// succeed normally.
    pub struct CasConflictOnceStore {
        pub inner: MockPromiseStore,
        conflict_fired: Arc<std::sync::atomic::AtomicBool>,
    }

    impl CasConflictOnceStore {
        pub fn new() -> Self {
            Self {
                inner: MockPromiseStore::new(),
                conflict_fired: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }
    }

    impl super::PromiseRepository for CasConflictOnceStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            let already = self
                .conflict_fired
                .swap(true, std::sync::atomic::Ordering::SeqCst);
            if !already {
                // First call: return a CAS conflict error without writing.
                return Box::pin(async move {
                    Err(super::PromiseStoreError(
                        "CAS mismatch: injected test conflict".to_string(),
                    ))
                });
            }
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Wraps [`MockPromiseStore`] and injects a CAS conflict on the first
    /// `update_promise` call. After the conflict fires, `get_promise` returns
    /// the promise with `status = Resolved`, simulating another worker that
    /// completed the run between our tool turn and the CAS write.
    ///
    /// Expected behaviour: `AgentLoop::run` detects the terminal status during
    /// the CAS reload and returns `Ok("")` immediately to avoid duplicating
    /// side-effects.
    pub struct TerminalOnCasReloadStore {
        pub inner: MockPromiseStore,
        conflict_fired: Arc<std::sync::atomic::AtomicBool>,
    }

    impl TerminalOnCasReloadStore {
        pub fn new() -> Self {
            Self {
                inner: MockPromiseStore::new(),
                conflict_fired: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }
    }

    impl super::PromiseRepository for TerminalOnCasReloadStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            let conflict_fired = Arc::clone(&self.conflict_fired);
            let inner = self.inner.clone();
            let tid = tenant_id.to_string();
            let pid = promise_id.to_string();
            Box::pin(async move {
                if conflict_fired.load(std::sync::atomic::Ordering::SeqCst) {
                    // After CAS conflict: another worker resolved the promise.
                    if let Some((mut p, rev)) = inner.get_promise(&tid, &pid).await? {
                        p.status = super::PromiseStatus::Resolved;
                        return Ok(Some((p, rev)));
                    }
                }
                inner.get_promise(&tid, &pid).await
            })
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            let already = self
                .conflict_fired
                .swap(true, std::sync::atomic::Ordering::SeqCst);
            if !already {
                return Box::pin(async move {
                    Err(super::PromiseStoreError(
                        "CAS mismatch: injected test conflict".to_string(),
                    ))
                });
            }
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Wraps [`MockPromiseStore`] and injects a CAS conflict on the first
    /// `update_promise` call. After the conflict fires, `get_promise` returns
    /// `Ok(None)`, simulating the promise key being deleted from KV between
    /// the tool turn and the CAS reload.
    ///
    /// Expected behaviour: `AgentLoop::run` sets `checkpoint = None`, disabling
    /// further checkpointing for the run, but the run still completes normally.
    pub struct DisappearedOnCasReloadStore {
        pub inner: MockPromiseStore,
        conflict_fired: Arc<std::sync::atomic::AtomicBool>,
    }

    impl DisappearedOnCasReloadStore {
        pub fn new() -> Self {
            Self {
                inner: MockPromiseStore::new(),
                conflict_fired: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }
    }

    impl super::PromiseRepository for DisappearedOnCasReloadStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            let conflict_fired = Arc::clone(&self.conflict_fired);
            let inner = self.inner.clone();
            let tid = tenant_id.to_string();
            let pid = promise_id.to_string();
            Box::pin(async move {
                if conflict_fired.load(std::sync::atomic::Ordering::SeqCst) {
                    // After CAS conflict: promise has been deleted from KV.
                    return Ok(None);
                }
                inner.get_promise(&tid, &pid).await
            })
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            let already = self
                .conflict_fired
                .swap(true, std::sync::atomic::Ordering::SeqCst);
            if !already {
                return Box::pin(async move {
                    Err(super::PromiseStoreError(
                        "CAS mismatch: injected test conflict".to_string(),
                    ))
                });
            }
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Wraps [`MockPromiseStore`] and makes every `update_promise` call hang
    /// indefinitely.
    ///
    /// Used with Tokio's mock clock (`start_paused = true`) to test that the
    /// `NATS_KV_TIMEOUT` on `update_promise` fires correctly and that the run
    /// sets `checkpoint = None`, disabling further checkpointing while still
    /// completing normally.
    pub struct HangingUpdateStore {
        pub inner: MockPromiseStore,
    }

    impl HangingUpdateStore {
        pub fn new() -> Self {
            Self {
                inner: MockPromiseStore::new(),
            }
        }
    }

    impl super::PromiseRepository for HangingUpdateStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
            _promise: &'a super::AgentPromise,
            _revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            Box::pin(std::future::pending())
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Wraps [`MockPromiseStore`] and makes every `get_tool_result` call hang
    /// indefinitely.
    ///
    /// Used with Tokio's mock clock to verify that the `NATS_KV_TIMEOUT` on
    /// `get_tool_result` fires correctly during recovery and that the timeout is
    /// treated as a cache miss — causing the tool to be re-executed rather than
    /// blocking the run.
    pub struct HangingGetToolResultStore {
        pub inner: MockPromiseStore,
    }

    impl HangingGetToolResultStore {
        pub fn new() -> Self {
            Self {
                inner: MockPromiseStore::new(),
            }
        }
    }

    impl super::PromiseRepository for HangingGetToolResultStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
            _cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            Box::pin(std::future::pending())
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Wraps [`MockPromiseStore`] and makes every `put_tool_result` call hang
    /// indefinitely.
    ///
    /// Used with Tokio's mock clock to verify that the `NATS_KV_TIMEOUT` on
    /// `put_tool_result` fires correctly and that the run continues normally —
    /// but the tool result is not persisted, so a subsequent crash would cause
    /// the tool to be re-executed on recovery.
    pub struct HangingPutToolResultStore {
        pub inner: MockPromiseStore,
    }

    impl HangingPutToolResultStore {
        pub fn new() -> Self {
            Self {
                inner: MockPromiseStore::new(),
            }
        }
    }

    impl super::PromiseRepository for HangingPutToolResultStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
            _cache_key: &'a str,
            _result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            Box::pin(std::future::pending())
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Makes every `get_promise` call hang indefinitely.
    ///
    /// Used with Tokio's mock clock to test that a timeout on the initial
    /// checkpoint load causes the run to start fresh rather than blocking.
    pub struct HangingGetPromiseStore {
        pub inner: MockPromiseStore,
    }

    impl HangingGetPromiseStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for HangingGetPromiseStore {
        fn get_promise<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            Box::pin(std::future::pending())
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Makes every `get_promise` call return an immediate `Err`.
    ///
    /// Used to test that a KV read error on the initial checkpoint load causes
    /// the run to start fresh rather than propagating the error to the caller.
    pub struct ErrorGetPromiseStore {
        pub inner: MockPromiseStore,
    }

    impl ErrorGetPromiseStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for ErrorGetPromiseStore {
        fn get_promise<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(super::PromiseStoreError("injected KV read error".to_string()))
            })
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Makes every `get_tool_result` call return an immediate `Err`.
    ///
    /// Used to verify that a KV read error during tool-result cache lookup is
    /// treated as a cache miss — the tool is re-executed rather than aborting.
    pub struct ErrorGetToolResultStore {
        pub inner: MockPromiseStore,
    }

    impl ErrorGetToolResultStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for ErrorGetToolResultStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
            _cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(super::PromiseStoreError("injected get_tool_result error".to_string()))
            })
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Makes every `put_tool_result` call return an immediate `Err`.
    ///
    /// Used to verify that a KV write error after tool execution is logged but
    /// does not abort the run — the tool result is simply not cached.
    pub struct ErrorPutToolResultStore {
        pub inner: MockPromiseStore,
    }

    impl ErrorPutToolResultStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for ErrorPutToolResultStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
            _cache_key: &'a str,
            _result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            Box::pin(async {
                Err(super::PromiseStoreError("injected put_tool_result error".to_string()))
            })
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Injects a CAS conflict on the first `update_promise`; subsequent
    /// `get_promise` calls (the CAS reload) return an immediate `Err`.
    ///
    /// Used to verify that a KV read error during CAS reload sets
    /// `checkpoint = None` and the run continues to completion.
    pub struct ErrorReloadStore {
        pub inner: MockPromiseStore,
        conflict_fired: Arc<std::sync::atomic::AtomicBool>,
    }

    impl ErrorReloadStore {
        pub fn new() -> Self {
            Self {
                inner: MockPromiseStore::new(),
                conflict_fired: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }
    }

    impl super::PromiseRepository for ErrorReloadStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            let fired = Arc::clone(&self.conflict_fired);
            let inner = self.inner.clone();
            let tid = tenant_id.to_string();
            let pid = promise_id.to_string();
            Box::pin(async move {
                if fired.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(super::PromiseStoreError(
                        "injected get_promise error during CAS reload".to_string(),
                    ));
                }
                inner.get_promise(&tid, &pid).await
            })
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            let already = self
                .conflict_fired
                .swap(true, std::sync::atomic::Ordering::SeqCst);
            if !already {
                return Box::pin(async {
                    Err(super::PromiseStoreError(
                        "CAS mismatch: injected test conflict".to_string(),
                    ))
                });
            }
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Wraps [`MockPromiseStore`] and injects a CAS conflict on the first
    /// `update_promise` call. After the conflict fires, subsequent `get_promise`
    /// calls hang indefinitely, simulating a NATS KV timeout during the CAS
    /// conflict reload.
    ///
    /// Used with Tokio's mock clock (`start_paused = true`) to exercise the
    /// `unwrap_or_else(|_| Ok(None))` path in the CAS reload branch — the
    /// timeout is converted to `Ok(None)`, which sets `checkpoint = None` and
    /// disables further checkpointing while the run still completes normally.
    pub struct HangingCasReloadStore {
        pub inner: MockPromiseStore,
        conflict_fired: Arc<std::sync::atomic::AtomicBool>,
    }

    impl HangingCasReloadStore {
        pub fn new() -> Self {
            Self {
                inner: MockPromiseStore::new(),
                conflict_fired: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }
    }

    impl super::PromiseRepository for HangingCasReloadStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            let conflict_fired = Arc::clone(&self.conflict_fired);
            let inner = self.inner.clone();
            let tid = tenant_id.to_string();
            let pid = promise_id.to_string();
            Box::pin(async move {
                if conflict_fired.load(std::sync::atomic::Ordering::SeqCst) {
                    // After CAS conflict: hang indefinitely to trigger the NATS_KV_TIMEOUT.
                    std::future::pending::<Result<Option<super::PromiseEntry>, super::PromiseStoreError>>().await
                } else {
                    inner.get_promise(&tid, &pid).await
                }
            })
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            let already = self
                .conflict_fired
                .swap(true, std::sync::atomic::Ordering::SeqCst);
            if !already {
                return Box::pin(async move {
                    Err(super::PromiseStoreError(
                        "CAS mismatch: injected conflict (hanging reload)".to_string(),
                    ))
                });
            }
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Makes every `put_promise` call hang indefinitely.
    ///
    /// Used with Tokio's mock clock to test that a timeout on the initial
    /// promise creation causes the run to proceed with promise fields still
    /// wired (rather than blocking forever).
    pub struct HangingPutPromiseStore {
        pub inner: MockPromiseStore,
    }

    impl HangingPutPromiseStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for HangingPutPromiseStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            _promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            Box::pin(std::future::pending())
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Makes every `put_promise` call return an immediate error.
    ///
    /// Used to test that a KV write failure during initial promise creation
    /// causes the run to proceed with promise fields still wired — the caller
    /// receives a valid agent and can attempt checkpoint writes on later turns.
    pub struct ErrorPutPromiseStore {
        pub inner: MockPromiseStore,
    }

    impl ErrorPutPromiseStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for ErrorPutPromiseStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            _promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            Box::pin(async {
                Err(super::PromiseStoreError("simulated put_promise failure".to_string()))
            })
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// Makes every `list_running` call hang indefinitely.
    ///
    /// Used with Tokio's mock clock to test that a timeout on `list_running`
    /// during startup recovery causes `recover_stale_promises` to return early
    /// without spawning a recovery task.
    pub struct HangingListRunningStore {
        pub inner: MockPromiseStore,
    }

    impl HangingListRunningStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for HangingListRunningStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            Box::pin(std::future::pending())
        }
    }

    /// Makes every `list_running` call return an immediate `Err`.
    ///
    /// Used to test that a KV error during startup recovery's `list_running`
    /// call causes `recover_stale_promises` to return early without spawning a
    /// recovery task.
    pub struct ErrorListRunningStore {
        pub inner: MockPromiseStore,
    }

    impl ErrorListRunningStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for ErrorListRunningStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(super::PromiseStoreError("injected list_running error".to_string()))
            })
        }
    }

    /// `list_running` delegates to inner (returns Running promises for the
    /// staleness scan), but `get_promise` always returns `Ok(None)`.
    ///
    /// Simulates the race where a promise appears in the startup scan but is
    /// deleted from KV before the recovery loop re-fetches it. Expected
    /// behaviour: the recovery loop logs and skips that promise without
    /// claiming or modifying it.
    pub struct VanishedOnRefetchStore {
        pub inner: MockPromiseStore,
    }

    impl VanishedOnRefetchStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for VanishedOnRefetchStore {
        fn get_promise<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            Box::pin(async { Ok(None) })
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// `list_running` delegates to inner (returns Running promises for the
    /// staleness scan), but `get_promise` always returns the promise with
    /// `status = Resolved`.
    ///
    /// Simulates the race where another worker completes the run between the
    /// startup scan and the re-fetch. Expected behaviour: the recovery loop
    /// logs "already completed" and skips without claiming.
    pub struct ResolvedOnRefetchStore {
        pub inner: MockPromiseStore,
    }

    impl ResolvedOnRefetchStore {
        pub fn new() -> Self {
            Self { inner: MockPromiseStore::new() }
        }
    }

    impl super::PromiseRepository for ResolvedOnRefetchStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<super::PromiseEntry>, super::PromiseStoreError>> + Send + 'a>>
        {
            let inner = self.inner.clone();
            let tid = tenant_id.to_string();
            let pid = promise_id.to_string();
            Box::pin(async move {
                if let Some((mut p, rev)) = inner.get_promise(&tid, &pid).await? {
                    p.status = super::PromiseStatus::Resolved;
                    Ok(Some((p, rev)))
                } else {
                    Ok(None)
                }
            })
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a super::AgentPromise,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a super::AgentPromise,
            revision: u64,
        ) -> Pin<Box<dyn Future<Output = Result<u64, super::PromiseStoreError>> + Send + 'a>> {
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), super::PromiseStoreError>> + Send + 'a>> {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<super::AgentPromise>, super::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
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

    /// `AgentPromise` with `system_prompt = Some(...)` must survive a serde
    /// round-trip, exercising the `Some` branch of the
    /// `#[serde(default)] pub system_prompt: Option<String>` field.
    /// The existing `agent_promise_round_trips_json` test only exercises
    /// `system_prompt: None`.
    #[test]
    fn agent_promise_serde_with_system_prompt_some() {
        let mut p = sample_promise("run-sp");
        p.system_prompt = Some("You are a helpful assistant.".to_string());

        let json = serde_json::to_string(&p).unwrap();
        assert!(
            json.contains("system_prompt"),
            "serialized promise must include the system_prompt key; got: {json}"
        );

        let back: AgentPromise = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.system_prompt,
            Some("You are a helpful assistant.".to_string()),
            "system_prompt Some variant must survive a serde round-trip"
        );
    }

    /// An `AgentPromise` serialized without the `system_prompt` key (e.g. by a
    /// previous binary that did not have the field) must deserialize cleanly
    /// with `system_prompt = None`.
    ///
    /// This exercises the `#[serde(default)]` annotation on the field —
    /// backward compatibility for promises written before the field was
    /// introduced.
    #[test]
    fn agent_promise_serde_default_system_prompt_when_key_absent() {
        // JSON produced by an older binary — no `system_prompt` key at all.
        let json = r#"{
            "id": "p-old",
            "tenant_id": "acme",
            "automation_id": "",
            "status": "running",
            "messages": [],
            "iteration": 0,
            "worker_id": "w",
            "claimed_at": 0,
            "trigger": null,
            "nats_subject": "github.pull_request"
        }"#;
        let p: AgentPromise = serde_json::from_str(json)
            .expect("AgentPromise must deserialize even without the system_prompt key");
        assert!(
            p.system_prompt.is_none(),
            "system_prompt must default to None when the key is absent from the serialized JSON"
        );
    }

    /// `put_promise` on a key that **already exists** must increment the stored
    /// revision rather than resetting it to 1.
    ///
    /// This exercises the `map(|(_, r)| r + 1)` branch of
    /// [`MockPromiseStore::put_promise`] — the only branch not covered by
    /// [`mock_put_and_get_promise`] (which always writes a fresh key).
    #[tokio::test]
    async fn mock_put_promise_on_existing_key_increments_revision() {
        let store = MockPromiseStore::new();
        let p = sample_promise("run-1");

        // First write: no existing key → revision starts at 1.
        let rev1 = store.put_promise(&p).await.unwrap();
        assert_eq!(rev1, 1, "first put must return revision 1");

        // Second write: key exists with revision 1 → must return 1 + 1 = 2.
        let mut updated = p.clone();
        updated.iteration = 1;
        let rev2 = store.put_promise(&updated).await.unwrap();
        assert_eq!(rev2, 2, "second put on existing key must increment revision to 2");

        // Verify the stored value and revision reflect the second write.
        let (fetched, stored_rev) = store.get_promise("acme", "run-1").await.unwrap().unwrap();
        assert_eq!(stored_rev, 2, "stored revision must be 2 after second put");
        assert_eq!(fetched.iteration, 1, "stored value must reflect the second put");
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

    /// When `update_promise` is called for a key that does not yet exist in the
    /// store, the mock's `_` arm inserts it as an upsert: `revision + 1` is
    /// returned and the promise is retrievable via `get_promise`.
    ///
    /// This exercises the `_` branch of the `match guard.get(&key)` in
    /// [`MockPromiseStore::update_promise`] — the only branch not covered by
    /// the existing `mock_update_promise_cas` test (which always operates on
    /// an existing key).
    #[tokio::test]
    async fn mock_update_promise_on_nonexistent_key_upserts() {
        let store = MockPromiseStore::new();
        let p = sample_promise("nonexistent");

        // No prior `put_promise` — the key does not exist.
        let new_rev = store
            .update_promise("acme", "nonexistent", &p, 0)
            .await
            .unwrap();
        assert_eq!(new_rev, 1, "upsert must return revision 0 + 1 = 1");

        let (fetched, rev) = store
            .get_promise("acme", "nonexistent")
            .await
            .unwrap()
            .expect("promise must be stored after upsert");
        assert_eq!(rev, 1, "stored revision must be 1");
        assert_eq!(fetched.id, "nonexistent", "stored promise id must match");
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

    /// `PromiseStoreError` implements `std::error::Error` with the default
    /// `source()` returning `None` — it has no underlying cause.
    /// Analogous to `agent_error_source_none_for_non_http` in `agent_loop`.
    #[test]
    fn promise_store_error_source_is_none() {
        let e = PromiseStoreError("some error".to_string());
        assert!(
            std::error::Error::source(&e).is_none(),
            "PromiseStoreError::source must be None — no underlying cause"
        );
    }

    // ── list_running ──────────────────────────────────────────────────────────

    /// `list_running` must return only promises with `status = Running`.
    /// Resolved, Failed, and PermanentFailed promises must be excluded.
    #[tokio::test]
    async fn mock_list_running_returns_only_running_promises() {
        let store = MockPromiseStore::new();

        let mut running = sample_promise("r1");
        running.status = PromiseStatus::Running;
        store.insert_promise(running);

        let mut resolved = sample_promise("r2");
        resolved.status = PromiseStatus::Resolved;
        store.insert_promise(resolved);

        let mut failed = sample_promise("r3");
        failed.status = PromiseStatus::Failed;
        store.insert_promise(failed);

        let mut perm_failed = sample_promise("r4");
        perm_failed.status = PromiseStatus::PermanentFailed;
        store.insert_promise(perm_failed);

        let results = store.list_running("acme").await.unwrap();
        assert_eq!(results.len(), 1, "only Running promises must be returned");
        assert_eq!(results[0].id, "r1");
    }

    /// `list_running` must filter by `tenant_id` prefix so that promises
    /// belonging to other tenants are never returned.
    #[tokio::test]
    async fn mock_list_running_filters_by_tenant_id() {
        let store = MockPromiseStore::new();

        // `sample_promise` hard-codes tenant_id = "acme".
        store.insert_promise(sample_promise("p1"));

        // A running promise owned by a different tenant.
        let other = AgentPromise {
            id: "p2".to_string(),
            tenant_id: "other-tenant".to_string(),
            automation_id: String::new(),
            status: PromiseStatus::Running,
            messages: vec![],
            iteration: 0,
            worker_id: "w".to_string(),
            claimed_at: 0,
            trigger: serde_json::Value::Null,
            nats_subject: "t".to_string(),
            system_prompt: None,
        };
        store.insert_promise(other);

        let acme = store.list_running("acme").await.unwrap();
        assert_eq!(acme.len(), 1);
        assert_eq!(acme[0].id, "p1", "must only return acme's promise");

        let other_results = store.list_running("other-tenant").await.unwrap();
        assert_eq!(other_results.len(), 1);
        assert_eq!(other_results[0].id, "p2", "must only return other-tenant's promise");

        let empty = store.list_running("unknown").await.unwrap();
        assert!(empty.is_empty(), "unknown tenant must return empty list");
    }
}

// ── Integration tests (real NATS KV via testcontainers) ───────────────────────

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Start a throwaway NATS container with JetStream and open a real
    /// `PromiseStore` against it.  Returns the store, the JetStream context
    /// (needed for the tombstone test), and an opaque drop guard that stops the
    /// container when it goes out of scope.
    async fn make_store() -> (PromiseStore, async_nats::jetstream::Context, impl Drop) {
        use testcontainers_modules::{
            nats::Nats,
            testcontainers::{ImageExt, runners::AsyncRunner},
        };
        let container = Nats::default()
            .with_cmd(["--jetstream"])
            .start()
            .await
            .expect("NATS container");
        let port = container.get_host_port_ipv4(4222).await.expect("NATS port");
        let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
            .await
            .expect("NATS connect");
        let js = async_nats::jetstream::new(nats);
        let store = PromiseStore::open(&js).await.expect("PromiseStore::open");
        (store, js, container)
    }

    fn make_promise(id: &str) -> AgentPromise {
        AgentPromise {
            id: id.to_string(),
            tenant_id: "tenant1".to_string(),
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

    /// `open()` must create both KV buckets so they are immediately accessible.
    #[tokio::test]
    async fn open_creates_both_buckets() {
        let (_, js, _c) = make_store().await;
        js.get_key_value(AGENT_PROMISES_BUCKET)
            .await
            .expect("AGENT_PROMISES bucket must exist after open()");
        js.get_key_value(AGENT_TOOL_RESULTS_BUCKET)
            .await
            .expect("AGENT_TOOL_RESULTS bucket must exist after open()");
    }

    /// Calling `open()` a second time on the same server must succeed — the
    /// underlying `create_or_update_key_value` is idempotent.
    #[tokio::test]
    async fn open_is_idempotent() {
        let (_, js, _c) = make_store().await;
        PromiseStore::open(&js)
            .await
            .expect("second open() must succeed");
    }

    /// `put_promise` followed by `get_promise` must return the original value
    /// and a positive KV revision.
    #[tokio::test]
    async fn put_and_get_promise_round_trip() {
        let (store, _, _c) = make_store().await;
        let p = make_promise("run-1");
        let rev = store.put_promise(&p).await.unwrap();
        assert!(rev >= 1, "revision must be at least 1");

        let (fetched, fetched_rev) = store
            .get_promise("tenant1", "run-1")
            .await
            .unwrap()
            .expect("promise must exist after put");
        assert_eq!(fetched.id, "run-1");
        assert_eq!(fetched.tenant_id, "tenant1");
        assert_eq!(fetched.status, PromiseStatus::Running);
        assert_eq!(fetched_rev, rev, "returned revision must match put() revision");
    }

    /// A second `put_promise` on the same key must overwrite the stored value.
    #[tokio::test]
    async fn put_promise_overwrites_existing() {
        let (store, _, _c) = make_store().await;
        let p = make_promise("run-overwrite");
        store.put_promise(&p).await.unwrap();

        let mut updated = p.clone();
        updated.iteration = 5;
        store.put_promise(&updated).await.unwrap();

        let (fetched, _) = store
            .get_promise("tenant1", "run-overwrite")
            .await
            .unwrap()
            .expect("must exist");
        assert_eq!(fetched.iteration, 5, "second put must overwrite the first");
    }

    /// `get_promise` on a key that has never been written must return `Ok(None)`.
    #[tokio::test]
    async fn get_missing_promise_returns_none() {
        let (store, _, _c) = make_store().await;
        let result = store
            .get_promise("tenant1", "nonexistent")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    /// `update_promise` with the correct revision must succeed and return a
    /// higher revision.  The stored value must reflect the update.
    #[tokio::test]
    async fn update_promise_cas_success() {
        let (store, _, _c) = make_store().await;
        let p = make_promise("run-cas");
        let rev1 = store.put_promise(&p).await.unwrap();

        let mut updated = p.clone();
        updated.iteration = 1;
        updated.status = PromiseStatus::Resolved;

        let rev2 = store
            .update_promise("tenant1", "run-cas", &updated, rev1)
            .await
            .unwrap();
        assert!(rev2 > rev1, "new revision must be greater than the old one");

        let (fetched, _) = store
            .get_promise("tenant1", "run-cas")
            .await
            .unwrap()
            .expect("must exist after update");
        assert_eq!(fetched.iteration, 1);
        assert_eq!(fetched.status, PromiseStatus::Resolved);
    }

    /// `update_promise` with a stale revision must return an error — the real
    /// NATS KV enforces the CAS check.
    #[tokio::test]
    async fn update_promise_cas_wrong_revision_fails() {
        let (store, _, _c) = make_store().await;
        let p = make_promise("run-cas-fail");
        let rev = store.put_promise(&p).await.unwrap();

        // Use a deliberately wrong revision.
        let stale_rev = rev + 999;
        let err = store
            .update_promise("tenant1", "run-cas-fail", &p, stale_rev)
            .await
            .unwrap_err();
        // The exact error message is NATS-specific; just assert it's non-empty.
        assert!(!err.to_string().is_empty(), "CAS mismatch must produce an error");
    }

    /// A two-writer CAS race: both workers read the same revision, only the
    /// first `update_promise` succeeds, the second must fail.
    #[tokio::test]
    async fn update_promise_cas_race_second_writer_fails() {
        let (store, _, _c) = make_store().await;
        let p = make_promise("run-race");
        let rev = store.put_promise(&p).await.unwrap();

        // First writer wins.
        let mut first = p.clone();
        first.worker_id = "worker-a".to_string();
        let new_rev = store
            .update_promise("tenant1", "run-race", &first, rev)
            .await
            .unwrap();
        assert!(new_rev > rev);

        // Second writer uses the original (now stale) revision — must fail.
        let mut second = p.clone();
        second.worker_id = "worker-b".to_string();
        let err = store
            .update_promise("tenant1", "run-race", &second, rev)
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty(), "second writer must lose the CAS race");

        // The stored value must be the first writer's update.
        let (fetched, _) = store
            .get_promise("tenant1", "run-race")
            .await
            .unwrap()
            .expect("must exist");
        assert_eq!(fetched.worker_id, "worker-a", "first writer's update must be stored");
    }

    /// `list_running` must return only promises with `status = Running`.
    #[tokio::test]
    async fn list_running_returns_running_only() {
        let (store, _, _c) = make_store().await;

        store.put_promise(&make_promise("r1")).await.unwrap();

        let mut resolved = make_promise("r2");
        resolved.status = PromiseStatus::Resolved;
        store.put_promise(&resolved).await.unwrap();

        let mut failed = make_promise("r3");
        failed.status = PromiseStatus::Failed;
        store.put_promise(&failed).await.unwrap();

        let mut perm = make_promise("r4");
        perm.status = PromiseStatus::PermanentFailed;
        store.put_promise(&perm).await.unwrap();

        let running = store.list_running("tenant1").await.unwrap();
        assert_eq!(running.len(), 1, "only Running promises must be returned");
        assert_eq!(running[0].id, "r1");
    }

    /// `list_running` must filter by `tenant_id` prefix and never return
    /// promises belonging to a different tenant.
    #[tokio::test]
    async fn list_running_filters_by_tenant() {
        let (store, _, _c) = make_store().await;

        store.put_promise(&make_promise("p1")).await.unwrap();

        let other = AgentPromise {
            id: "p2".to_string(),
            tenant_id: "other-tenant".to_string(),
            automation_id: String::new(),
            status: PromiseStatus::Running,
            messages: vec![],
            iteration: 0,
            worker_id: "w".to_string(),
            claimed_at: 0,
            trigger: serde_json::Value::Null,
            nats_subject: "t".to_string(),
            system_prompt: None,
        };
        store.put_promise(&other).await.unwrap();

        let tenant1 = store.list_running("tenant1").await.unwrap();
        assert_eq!(tenant1.len(), 1);
        assert_eq!(tenant1[0].id, "p1");

        let other_results = store.list_running("other-tenant").await.unwrap();
        assert_eq!(other_results.len(), 1);
        assert_eq!(other_results[0].id, "p2");

        let unknown = store.list_running("unknown-tenant").await.unwrap();
        assert!(unknown.is_empty(), "unknown tenant must return empty list");
    }

    /// A key deleted from the KV bucket creates a tombstone.  `list_running`
    /// must skip tombstones and not attempt to deserialize them.
    #[tokio::test]
    async fn list_running_skips_tombstone() {
        let (store, js, _c) = make_store().await;

        // Write a Running promise.
        store.put_promise(&make_promise("to-delete")).await.unwrap();

        // Verify it appears in list_running.
        let before = store.list_running("tenant1").await.unwrap();
        assert_eq!(before.len(), 1, "promise must appear before deletion");

        // Delete the key directly via the raw KV bucket to create a tombstone.
        let kv = js
            .get_key_value(AGENT_PROMISES_BUCKET)
            .await
            .expect("bucket must be accessible");
        kv.delete("tenant1.to-delete")
            .await
            .expect("delete must succeed");

        // list_running must skip the tombstone entry and return empty.
        let after = store.list_running("tenant1").await.unwrap();
        assert!(
            after.is_empty(),
            "list_running must skip tombstone entries — got {:?}",
            after.iter().map(|p| &p.id).collect::<Vec<_>>()
        );
    }

    /// `put_tool_result` followed by `get_tool_result` must return the original
    /// string unchanged.
    #[tokio::test]
    async fn tool_result_round_trip() {
        let (store, _, _c) = make_store().await;
        let cache_key = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        store
            .put_tool_result("tenant1", "run-1", cache_key, "tool result text")
            .await
            .unwrap();
        let result = store
            .get_tool_result("tenant1", "run-1", cache_key)
            .await
            .unwrap();
        assert_eq!(result, Some("tool result text".to_string()));
    }

    /// `get_tool_result` for a key that was never written must return `Ok(None)`.
    #[tokio::test]
    async fn get_missing_tool_result_returns_none() {
        let (store, _, _c) = make_store().await;
        let result = store
            .get_tool_result(
                "tenant1",
                "run-1",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .await
            .unwrap();
        assert!(result.is_none());
    }

    /// A second `put_tool_result` on the same key must overwrite the first value.
    #[tokio::test]
    async fn put_tool_result_overwrites_existing() {
        let (store, _, _c) = make_store().await;
        let cache_key = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        store
            .put_tool_result("tenant1", "run-1", cache_key, "first result")
            .await
            .unwrap();
        store
            .put_tool_result("tenant1", "run-1", cache_key, "second result")
            .await
            .unwrap();
        let result = store
            .get_tool_result("tenant1", "run-1", cache_key)
            .await
            .unwrap();
        assert_eq!(
            result,
            Some("second result".to_string()),
            "second put must overwrite the first"
        );
    }

    /// Tool results are scoped to `(tenant_id, promise_id, cache_key)`.
    /// A different `promise_id` must not see another promise's cached result.
    #[tokio::test]
    async fn tool_result_scoped_to_promise() {
        let (store, _, _c) = make_store().await;
        let cache_key = "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe";
        store
            .put_tool_result("tenant1", "run-A", cache_key, "result-A")
            .await
            .unwrap();

        // A different promise must not see run-A's cached result.
        let result = store
            .get_tool_result("tenant1", "run-B", cache_key)
            .await
            .unwrap();
        assert!(
            result.is_none(),
            "run-B must not see run-A's tool result; got: {result:?}"
        );
    }
}
