//! Core agentic loop: prompt → Anthropic (via proxy) → tool calls → repeat.
//!
//! The loop follows the Anthropic tool-use protocol:
//! 1. Send `messages` + `tools` to the model.
//! 2. If `stop_reason == "end_turn"` → return the text output.
//! 3. If `stop_reason == "tool_use"` → execute each requested tool, append
//!    results, and send another request.
//! 4. Repeat until `end_turn` or `max_iterations` is reached.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};

use crate::flag_client::FeatureFlagClient;
use crate::promise_store::PromiseRepository;
use crate::tools::{ToolDef, ToolDispatcher};
use trogon_agent_core::agent_loop::{ElicitationProvider, PermissionChecker};

// ── AnthropicClient trait ──────────────────────────────────────────────────────

/// Error returned by [`AnthropicClient::complete`].
///
/// `Permanent` means the request was rejected with a non-retryable 4xx — retrying
/// won't help. `Transient` covers transport failures, timeouts, 5xx, and 429 —
/// the upstream may recover on a subsequent attempt.
#[derive(Debug, Clone)]
pub enum AnthropicClientError {
    Permanent(String),
    Transient(String),
}

impl std::fmt::Display for AnthropicClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Permanent(msg) | Self::Transient(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for AnthropicClientError {}

/// Trait for sending a request to the Anthropic messages API.
pub trait AnthropicClient: Send + Sync + 'static {
    fn complete<'a>(
        &'a self,
        body: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>;
}

/// Concrete [`AnthropicClient`] backed by a [`reqwest::Client`].
pub struct ReqwestAnthropicClient {
    http: reqwest::Client,
    proxy_url: String,
    anthropic_token: String,
}

impl ReqwestAnthropicClient {
    pub fn new(http: reqwest::Client, proxy_url: String, anthropic_token: String) -> Self {
        Self {
            http,
            proxy_url,
            anthropic_token,
        }
    }
}

impl AnthropicClient for ReqwestAnthropicClient {
    fn complete<'a>(
        &'a self,
        body: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
    {
        Box::pin(async move {
            // Retry up to 3 times on transient errors. Backoff: 2s → 4s → 8s.
            //
            // Two classes of retryable error:
            // - Transport errors (`reqwest::Error`): connection reset, timeout,
            //   DNS failure, incomplete response body.
            // - HTTP 429 (rate limit): handled explicitly before
            //   `error_for_status()` — reads the `Retry-After` header and
            //   waits that many seconds (default: 60 s) before retrying.
            // - HTTP 5xx (server error): `error_for_status()` converts to
            //   `reqwest::Error` so they flow through the same retry path as
            //   transport errors.
            //
            // Non-retryable 4xx (400, 401, 403) also become `reqwest::Error`
            // via `error_for_status()` but `is_retryable_error` returns false
            // for those status codes, so they propagate as Permanent errors.
            let mut attempts = 0u32;
            loop {
                let send_result = self
                    .http
                    .post(format!("{}/anthropic/v1/messages", self.proxy_url))
                    .header("Authorization", format!("Bearer {}", self.anthropic_token))
                    .header("anthropic-version", "2023-06-01")
                    // Hard cap per LLM call. Without this, a hung Anthropic API
                    // keeps the heartbeat alive indefinitely and the run never
                    // completes.
                    .timeout(std::time::Duration::from_secs(5 * 60))
                    .json(&body)
                    .send()
                    .await;

                let response = match send_result {
                    Ok(r) => r,
                    Err(e) if attempts < 3 && is_retryable_error(&e) => {
                        attempts += 1;
                        tracing::warn!(
                            attempt = attempts,
                            error = %e,
                            "Anthropic request failed with retryable error — retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1 << attempts)).await;
                        continue;
                    }
                    Err(e) => return Err(AnthropicClientError::Transient(e.to_string())),
                };

                // 429: respect Retry-After header (default: 60 s).
                // Handled before error_for_status() so the header is still
                // accessible on the response.
                if response.status() == 429 && attempts < 3 {
                    let delay = retry_after_delay(response.headers());
                    attempts += 1;
                    tracing::warn!(
                        attempt = attempts,
                        delay_secs = delay,
                        "Anthropic rate limited (429) — waiting Retry-After before retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    continue;
                }

                // Convert 4xx/5xx HTTP status codes to errors so the retry
                // machinery can handle server errors (5xx).
                let response = match response.error_for_status() {
                    Ok(r) => r,
                    Err(e) if attempts < 3 && is_retryable_error(&e) => {
                        attempts += 1;
                        tracing::warn!(
                            attempt = attempts,
                            error = %e,
                            "Anthropic returned retryable HTTP status — retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1 << attempts)).await;
                        continue;
                    }
                    Err(e) => {
                        return Err(reqwest_error_to_anthropic_error(e));
                    }
                };

                match response.json::<serde_json::Value>().await {
                    Ok(v) => return Ok(v),
                    Err(e) if attempts < 3 && is_retryable_error(&e) => {
                        attempts += 1;
                        tracing::warn!(
                            attempt = attempts,
                            error = %e,
                            "Anthropic response read failed with retryable error — retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1 << attempts)).await;
                    }
                    Err(e) => return Err(AnthropicClientError::Transient(e.to_string())),
                }
            }
        })
    }
}

/// Returns `true` for errors that are safe to retry.
///
/// Covers two categories:
/// - Transport errors: connection failures, timeouts, incomplete responses.
/// - HTTP 5xx (server error): transient Anthropic-side conditions that resolve
///   without code changes. Converted to `reqwest::Error` by `error_for_status()`
///   before reaching this function.
///
/// HTTP 429 (rate limit) is handled separately — before `error_for_status()` is
/// called — so it never reaches this function. Non-retryable 4xx (400, 401, 403)
/// return `false`: those indicate a broken request that won't be fixed by retrying.
fn is_retryable_error(e: &reqwest::Error) -> bool {
    e.is_connect()
        || e.is_timeout()
        || e.is_request()
        || e.status().map(|s| s.is_server_error()).unwrap_or(false)
}

/// Converts a `reqwest::Error` (from `error_for_status`) into `AnthropicClientError`.
///
/// 4xx errors (except 429, which is retried before reaching this) are `Permanent`
/// since retrying the same broken request won't help. Everything else is `Transient`.
fn reqwest_error_to_anthropic_error(e: reqwest::Error) -> AnthropicClientError {
    let is_permanent = e
        .status()
        .map(|s| s.is_client_error() && s != 429)
        .unwrap_or(false);
    if is_permanent {
        AnthropicClientError::Permanent(e.to_string())
    } else {
        AnthropicClientError::Transient(e.to_string())
    }
}

/// Parse the `Retry-After` header value as a number of seconds.
///
/// Anthropic uses integer-seconds format (e.g. `Retry-After: 30`). If the
/// header is absent, unparseable, or contains an HTTP-date string, returns
/// `60` as a conservative default.
fn retry_after_delay(headers: &reqwest::header::HeaderMap) -> u64 {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60)
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    pub struct MockAnthropicClient {
        pub response: serde_json::Value,
    }

    impl AnthropicClient for MockAnthropicClient {
        fn complete<'a>(
            &'a self,
            _body: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
        {
            let resp = self.response.clone();
            Box::pin(async move { Ok(resp) })
        }
    }

    /// Mock that returns responses from a pre-loaded queue, in order.
    ///
    /// Panics if called when the queue is empty — useful for asserting Anthropic
    /// is never called (pass an empty `Vec`).
    ///
    /// All request bodies passed to `complete` are recorded in `captured_bodies`
    /// so tests can assert on what was sent to the API.
    pub struct SequencedMockAnthropicClient {
        responses: Mutex<VecDeque<serde_json::Value>>,
        captured: Mutex<Vec<serde_json::Value>>,
    }

    impl SequencedMockAnthropicClient {
        pub fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                captured: Mutex::new(Vec::new()),
            }
        }

        /// Returns all request bodies received by `complete`, in call order.
        pub fn captured_bodies(&self) -> Vec<serde_json::Value> {
            self.captured.lock().unwrap().clone()
        }
    }

    impl AnthropicClient for SequencedMockAnthropicClient {
        fn complete<'a>(
            &'a self,
            body: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
        {
            self.captured.lock().unwrap().push(body);
            let resp = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("SequencedMockAnthropicClient ran out of queued responses");
            Box::pin(async move { Ok(resp) })
        }
    }

    /// An `AnthropicClient` that always returns a transient error — used to
    /// test graceful degradation paths that call the LLM and handle failure.
    pub struct AlwaysErrAnthropicClient;

    impl AnthropicClient for AlwaysErrAnthropicClient {
        fn complete<'a>(
            &'a self,
            _body: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
        {
            Box::pin(async move { Err(AnthropicClientError::Transient("simulated error".into())) })
        }
    }
}

// ── Wire types ────────────────────────────────────────────────────────────────

/// Token usage reported by the Anthropic API for a single response.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

/// A single message in the Anthropic conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
    // Persisted in NATS KV but never sent to Anthropic — the API returns 400
    // for extra fields. `AnthropicMessageRef` wraps Message for the request
    // and omits this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Serialization wrapper for `Message` that omits `usage`.
///
/// Anthropic returns 400 "extra inputs are not permitted" when `messages`
/// contains a `usage` field. This newtype is used in `AnthropicRequest` so
/// `usage` is silently dropped on the wire while still being persisted in NATS KV.
#[derive(Debug)]
struct AnthropicMessageRef<'a>(&'a Message);

impl serde::Serialize for AnthropicMessageRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("Message", 2)?;
        s.serialize_field("role", &self.0.role)?;
        s.serialize_field("content", &self.0.content)?;
        s.end()
    }
}

impl Message {
    /// Simple user turn with plain text.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: text.into() }],
            usage: None,
        }
    }

    /// Assistant turn (used when appending a model response to history).
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            usage: None,
        }
    }

    /// User turn carrying `tool_result` blocks.
    pub fn tool_results(results: Vec<ToolResult>) -> Self {
        Self {
            role: "user".to_string(),
            content: results
                .into_iter()
                .map(|r| ContentBlock::ToolResult {
                    tool_use_id: r.tool_use_id,
                    content: r.content,
                })
                .collect(),
            usage: None,
        }
    }
}

/// A single block within a message's `content` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text from the model or the user.
    Text { text: String },
    /// Tool invocation requested by the model.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Result returned to the model after executing a tool.
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// Pair of tool-use ID and the string result to feed back to the model.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
}

/// A single block in the Anthropic `system` array.
///
/// Using an array (rather than a plain string) allows `cache_control` to be
/// attached, which enables prompt caching on the system prompt.
#[derive(Debug, Serialize)]
struct SystemBlock<'a> {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: &'a str,
    cache_control: CacheControl,
}

/// Anthropic prompt-caching control block (`{"type":"ephemeral"}`).
#[derive(Debug, Clone, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: &'static str,
}

impl CacheControl {
    const fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral",
        }
    }
}

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    /// System prompt sent as a cacheable content block.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<SystemBlock<'a>>>,
    tools: &'a [ToolDef],
    messages: Vec<AnthropicMessageRef<'a>>,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    stop_reason: String,
    content: Vec<ContentBlock>,
    #[serde(default)]
    usage: Option<Usage>,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AgentError {
    Http(AnthropicClientError),
    MaxIterationsReached,
    UnexpectedStopReason(String),
    /// NATS KV timed out on the initial `get_promise` load after all retries.
    ///
    /// The promise is left in `Running` state so startup recovery can pick it
    /// up once KV is healthy again. The run does not start — returning an error
    /// here is safer than proceeding without checkpoint durability and
    /// potentially re-executing tools that were already cached.
    CheckpointLoadTimeout,
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::MaxIterationsReached => write!(f, "Agent exceeded max iterations"),
            Self::UnexpectedStopReason(r) => write!(f, "Unexpected stop reason: {r}"),
            Self::CheckpointLoadTimeout => write!(
                f,
                "NATS KV timed out loading checkpoint — run deferred to startup recovery"
            ),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Http(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

// ── AgentLoop ─────────────────────────────────────────────────────────────────

/// Runs the Anthropic tool-use loop, routing all AI calls through the proxy.
#[derive(Clone)]
pub struct AgentLoop {
    /// Client for calling the Anthropic messages API (via proxy).
    pub anthropic_client: Arc<dyn AnthropicClient>,
    pub model: String,
    pub max_iterations: u32,
    /// Dispatcher for built-in tool calls.
    pub tool_dispatcher: Arc<dyn ToolDispatcher>,
    /// Shared HTTP context used by handlers for non-tool operations (e.g.
    /// fetching the memory file from GitHub).
    pub tool_context: Arc<dyn crate::tools::AgentConfig>,
    /// GitHub repo owner for pre-fetching the memory file in handlers
    /// that don't have an implicit repo (e.g. Linear issue triage).
    pub memory_owner: Option<String>,
    /// GitHub repo name for pre-fetching the memory file.
    pub memory_repo: Option<String>,
    /// Path of the memory file inside the repository.
    /// Defaults to `.trogon/memory.md` when `None`.
    pub memory_path: Option<String>,
    /// Extra tool definitions from MCP servers — appended to every `run` call.
    pub mcp_tool_defs: Vec<ToolDef>,
    /// Dispatch map for MCP tools: prefixed_name → (client, original_tool_name).
    pub mcp_dispatch: Vec<(String, String, Arc<dyn trogon_mcp::McpCallTool>)>,
    /// Feature flag client for evaluating flags per tenant.
    pub flag_client: Arc<dyn FeatureFlagClient>,
    /// Tenant identifier used as the feature flag evaluation key.
    pub tenant_id: String,
    /// Durable promise store for checkpointing run state across process restarts.
    /// `None` disables durability (backward-compatible default).
    pub promise_store: Option<Arc<dyn PromiseRepository>>,
    /// Unique identifier for the current run, used as the KV checkpoint key.
    /// `None` when `promise_store` is `None`.
    pub promise_id: Option<String>,
    /// Optional gate called before each tool execution — `None` means all tools are auto-allowed.
    pub permission_checker: Option<Arc<dyn PermissionChecker>>,
    /// Optional provider for the built-in `ask_user` tool; `None` means the tool is not offered.
    pub elicitation_provider: Option<Arc<dyn ElicitationProvider>>,
}

/// Recursively sort JSON object keys so that serialization is order-independent.
///
/// `serde_json::Value` preserves insertion order (IndexMap). The LLM can produce
/// the same logical input with different key ordering across requests — in
/// particular on crash recovery, when it regenerates a fresh request from the
/// same context. Sorting object keys before hashing makes the cache key identical
/// for semantically equivalent inputs regardless of key order.
fn sort_json_keys(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            // BTreeMap iterates in sorted key order; collect back into a
            // serde_json::Map (IndexMap) which preserves insertion order, so
            // to_string() will emit keys alphabetically.
            let sorted: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_json_keys(v)))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect();
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_json_keys).collect()),
        other => other.clone(),
    }
}

/// Compute a stable cache key for a tool call from its name and input.
///
/// Uses SHA-256 of `"{tool_name}:{canonical_json(input)}"` where
/// `canonical_json` sorts all object keys alphabetically before serializing.
/// This makes the key order-independent: the LLM can regenerate the same
/// logical call with different key ordering on crash recovery and the hash
/// will still match, replaying the cached result without re-executing the tool.
fn tool_cache_key(tool_name: &str, input: &Value) -> String {
    let canonical = format!(
        "{tool_name}:{}",
        serde_json::to_string(&sort_json_keys(input)).unwrap_or_default()
    );
    let digest = Sha256::digest(canonical.as_bytes());
    format!("{digest:x}")
}

/// Timeout for NATS KV write operations.
///
/// NATS is a local-network hop, so writes complete in milliseconds under normal
/// conditions. A 10-second timeout absorbs transient server hiccups without
/// blocking the run indefinitely under NATS degradation. When the timeout fires
/// the checkpoint attempt is abandoned — the heartbeat task keeps the NATS
/// message alive, so the run continues; worst case is one extra re-execution on
/// the next crash recovery.
pub(crate) const NATS_KV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Hard cap per tool call.
///
/// External APIs (GitHub, Slack, Linear) normally respond in under 5 seconds.
/// 60 seconds is generous enough to absorb unusually slow responses while
/// ensuring a hung API does not block the run indefinitely — the heartbeat
/// task would otherwise keep the NATS message alive forever and the run would
/// never complete or fail.
///
/// On timeout the tool returns an error string to the model so the agent can
/// decide how to proceed (retry, skip, or summarise the failure). The run is
/// not aborted.
pub(crate) const TOOL_EXECUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Maximum serialized size of an [`AgentPromise`] checkpoint written to NATS KV.
///
/// NATS KV defaults to a 1 MB value size limit. Exceeding it causes the write
/// to be silently rejected, leaving a stale revision and making the run
/// non-recoverable. 768 KB leaves ~25% headroom for NATS framing overhead and
/// for growth in the promise envelope fields.
///
/// When a checkpoint would exceed this limit, checkpointing is disabled for
/// the remainder of the run. The last successful checkpoint is still valid for
/// crash recovery — the worst case is re-executing the turns since that
/// checkpoint.
pub(crate) const CHECKPOINT_MAX_BYTES: usize = 768 * 1024;

/// How often the agent refreshes `claimed_at` in KV before starting each LLM
/// call.
///
/// 3 minutes keeps the heartbeat well under the 15-minute stale threshold used
/// by startup recovery, with generous margin even if two consecutive heartbeats
/// miss.
///
/// In test builds this is overridden to [`std::time::Duration::ZERO`] so the
/// heartbeat fires on every loop iteration without needing to wait 3 real
/// minutes.
#[cfg(not(test))]
const HEARTBEAT_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3 * 60);
#[cfg(test)]
const HEARTBEAT_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::ZERO;

/// Write a terminal status (`Failed` or `PermanentFailed`) to KV, with a
/// `NATS_KV_TIMEOUT` deadline.
///
/// All error paths in `AgentLoop::run` call this so a stuck-`Running` promise
/// is never left for the startup recovery scan to retry indefinitely.
///
/// ## Which status to use
///
/// - [`PromiseStatus::Failed`] — transient errors (HTTP timeout, NATS hiccup).
///   NATS redelivery will retry the run from its checkpoint.
/// - [`PromiseStatus::PermanentFailed`] — deterministic errors that retrying
///   cannot fix (`max_tokens`, `MaxIterationsReached`, unknown `stop_reason`).
///   Neither startup recovery nor NATS redelivery will re-run the promise.
///
/// `context` is a short label included in the warning log.
async fn write_promise_terminal(
    store: &dyn PromiseRepository,
    tenant_id: &str,
    pid: &str,
    promise: &mut crate::promise_store::AgentPromise,
    rev: u64,
    status: crate::promise_store::PromiseStatus,
    context: &str,
) {
    promise.status = status;
    promise.failure_reason = Some(context.to_string());
    match tokio::time::timeout(
        NATS_KV_TIMEOUT,
        store.update_promise(tenant_id, pid, promise, rev),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            // CAS conflict — the stored revision is stale. This happens when
            // `checkpointing_disabled = true`: message-history checkpoints are
            // skipped, but the pre-LLM heartbeat still advances the KV revision
            // without updating the local copy. Reload the current revision and
            // retry once so the terminal status is durably written.
            warn!(error = %e, context, "Terminal write CAS conflict — reloading revision and retrying");
            match tokio::time::timeout(NATS_KV_TIMEOUT, store.get_promise(tenant_id, pid)).await {
                Ok(Ok(Some((_, fresh_rev)))) => {
                    match tokio::time::timeout(
                        NATS_KV_TIMEOUT,
                        store.update_promise(tenant_id, pid, promise, fresh_rev),
                    )
                    .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            error!(error = %e, context, promise_id = %pid, "Failed to write terminal promise status after revision reload — promise will stay Running until TTL")
                        }
                        Err(_) => {
                            error!(promise_id = %pid, context, "NATS KV timed out writing terminal promise status after revision reload — promise will stay Running until TTL")
                        }
                    }
                }
                Ok(Ok(None)) => {
                    warn!(promise_id = %pid, context, "Promise vanished from KV — terminal status write skipped");
                }
                Ok(Err(e)) => {
                    error!(error = %e, context, promise_id = %pid, "Could not reload KV revision for terminal write retry — promise will stay Running until TTL");
                }
                Err(_) => {
                    error!(promise_id = %pid, context, "KV timeout reloading revision for terminal write retry — promise will stay Running until TTL");
                }
            }
        }
        Err(_) => {
            error!(promise_id = %pid, context, "NATS KV write timed out writing terminal promise status — promise will stay Running until TTL")
        }
    }
}

/// When `checkpoint` is `None` (the CAS revision was lost to a reload error)
/// and the run ends with a *deterministic* error, attempt to mark the promise
/// `PermanentFailed` via a fresh `get_promise` + CAS write.
///
/// Without this, the promise stays `Running` and startup recovery re-runs it
/// on every restart — wasting resources on a hopeless run that will always
/// hit the same deterministic error.
///
/// Retries up to 5 times with exponential back-off (2 s, 4 s, 8 s, 16 s)
/// to survive transient NATS hiccups that may have caused the checkpoint
/// revision to be lost in the first place. After 5 failures the promise
/// stays `Running` and will be recovered correctly once NATS comes back
/// online.
async fn try_mark_permanent_failed_fresh(
    store: &dyn PromiseRepository,
    tenant_id: &str,
    pid: &str,
    context: &str,
) {
    for attempt in 0u32..5 {
        if attempt > 0 {
            // Exponential back-off: 2 s, 4 s, 8 s, 16 s.
            tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
        }
        match tokio::time::timeout(NATS_KV_TIMEOUT, store.get_promise(tenant_id, pid)).await {
            Ok(Ok(Some((mut p, rev)))) => {
                write_promise_terminal(
                    store,
                    tenant_id,
                    pid,
                    &mut p,
                    rev,
                    crate::promise_store::PromiseStatus::PermanentFailed,
                    context,
                )
                .await;
                return; // write attempted (success or terminal CAS failure — either way, stop)
            }
            Ok(Ok(None)) => return, // Promise gone from KV — no cycle risk
            Ok(Err(e)) => {
                warn!(error = %e, context, attempt, "Could not fetch promise for terminal-status write — retrying")
            }
            Err(_) => warn!(
                context,
                attempt, "KV timeout fetching promise for terminal-status write — retrying"
            ),
        }
    }
    error!(context, promise_id = %pid, "Failed to mark promise PermanentFailed after 5 attempts — promise will stay Running until TTL or next successful recovery");
}

/// Render a slice of messages as readable text for the summarization prompt.
///
/// Tool calls are shown as `name: {input}` and results as `Tool result: …`
/// so the LLM gets a human-readable transcript of both sides of each turn.
fn render_messages_for_summary(messages: &[Message]) -> String {
    let mut out = String::new();
    for msg in messages {
        let role = match msg.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            other => other,
        };
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    out.push_str(role);
                    out.push_str(": ");
                    out.push_str(text);
                    out.push_str("\n\n");
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    out.push_str(&format!("{role} called tool `{name}`: {input}\n\n"));
                }
                ContentBlock::ToolResult { content, .. } => {
                    out.push_str("Tool result: ");
                    out.push_str(content);
                    out.push_str("\n\n");
                }
            }
        }
    }
    out
}

/// Call the LLM to summarize `dropped` messages that are about to be trimmed
/// from the checkpoint.
///
/// Returns `[user_summary_msg, assistant_ack_msg]` on success — a synthetic
/// pair that can be prepended to the kept messages so the resumed run retains
/// semantic context for earlier decisions.
///
/// Returns an empty `Vec` on any failure (network error, deserialization
/// error, empty LLM response) so callers can fall back to plain trimming.
async fn summarize_dropped_messages(
    client: &dyn AnthropicClient,
    model: &str,
    dropped: &[Message],
) -> Vec<Message> {
    let rendered = render_messages_for_summary(dropped);
    if rendered.is_empty() {
        return vec![];
    }
    let prompt = format!(
        "The following is the beginning of a conversation that is being truncated due to length. \
         Summarize it concisely in plain text, preserving all key decisions, findings, discovered \
         facts, and any context that would be needed to continue the work effectively. \
         Focus on outcomes and important information — omit conversational phrasing.\n\n{rendered}"
    );
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": prompt}]
    });
    let raw = match client.complete(body).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "LLM summarization call failed — will use plain history trim");
            return vec![];
        }
    };
    let resp = match serde_json::from_value::<AnthropicResponse>(raw) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "LLM summarization response deserialization failed — will use plain history trim");
            return vec![];
        }
    };
    let text: String = resp
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return vec![];
    }
    vec![
        Message::user_text(format!(
            "Context summary from earlier in this conversation (some history was truncated):\n\n{text}"
        )),
        Message::assistant(vec![ContentBlock::Text {
            text: "Understood. I have the context from the earlier part of this conversation \
                   and will keep it in mind."
                .to_string(),
        }]),
    ]
}

impl AgentLoop {
    /// Check whether a feature flag is enabled for this agent's tenant.
    ///
    /// Delegates to the injected [`FeatureFlagClient`].  When an
    /// [`AlwaysOnFlagClient`] is configured (no Split.io), all flags return
    /// `true` — fail-open ensures all handlers run by default.
    ///
    /// [`AlwaysOnFlagClient`]: crate::flag_client::AlwaysOnFlagClient
    pub async fn is_flag_enabled(&self, flag: &dyn trogon_splitio::flags::FeatureFlag) -> bool {
        self.flag_client.is_enabled(&self.tenant_id, flag).await
    }

    /// Run the agentic loop starting from `initial_messages`.
    ///
    /// `system_prompt` is injected as the Anthropic `system` field — use it to
    /// provide persistent memory (e.g. the contents of `.trogon/memory.md`).
    /// Pass `None` when no system prompt is needed.
    ///
    /// Returns the final text produced by the model when it stops requesting
    /// tools.
    pub async fn run(
        &self,
        initial_messages: Vec<Message>,
        tools: &[ToolDef],
        system_prompt: Option<&str>,
    ) -> Result<String, AgentError> {
        let mut messages = initial_messages;

        // ── Durable promise: load checkpoint ─────────────────────────────────
        // If a promise store and promise ID are configured, check KV for an
        // existing checkpoint. If one exists with a non-empty message history,
        // resume from it rather than starting from scratch.
        let mut checkpoint: Option<(crate::promise_store::AgentPromise, u64)> = None;
        // When `true`, message-history checkpoint writes are skipped (payload too
        // large for NATS KV), but terminal-status writes (Resolved, PermanentFailed)
        // still proceed using the revision stored in `checkpoint`. Distinct from
        // `checkpoint = None`, which means the KV revision was genuinely lost and
        // no writes of any kind are possible.
        // `checkpointing_disabled` is declared inside the `tool_use` match arm so
        // it resets to false on every iteration — re-attempt checkpointing each turn
        // in case the payload has shrunk since it last exceeded the limit.
        // `recovering` is true when we loaded a non-empty checkpoint — used only for
        // system-prompt pinning logic. Tool-result cache is gated on `promise_id`
        // being set (see `execute_tools`), not on this flag.
        let mut recovering = false;
        if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
            // Attempt the initial checkpoint load twice before giving up.
            // A single transient NATS blip should not forfeit all checkpoint
            // durability for the run — one retry covers the common case.
            // If both attempts time out we return Err rather than proceeding
            // without a checkpoint: running without durability risks
            // re-executing tool calls that were already cached in a previous
            // delivery, which is worse than deferring to startup recovery.
            // The promise remains Running so recover_stale_promises will pick
            // it up on the next restart once KV is healthy again.
            let initial_load = 'load: {
                let mut last_err = None;
                for attempt in 0u32..2 {
                    match tokio::time::timeout(
                        NATS_KV_TIMEOUT,
                        store.get_promise(&self.tenant_id, pid),
                    )
                    .await
                    {
                        Ok(r) => break 'load r,
                        Err(_) => {
                            warn!(
                                promise_id = %pid,
                                attempt,
                                "NATS KV get_promise timed out on initial load"
                            );
                            last_err = Some(());
                            if attempt == 0 {
                                // Brief pause before retrying — gives a
                                // momentarily overloaded KV server time to
                                // recover without burning the full timeout again.
                                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                            }
                        }
                    }
                }
                let _ = last_err;
                error!(
                    promise_id = %pid,
                    "NATS KV get_promise timed out on initial load after 2 attempts \
                     — aborting run; promise stays Running for startup recovery"
                );
                return Err(AgentError::CheckpointLoadTimeout);
            };
            match initial_load {
                Ok(Some((p, rev))) => {
                    // If the promise is already Resolved, the run completed
                    // successfully on a previous attempt. This path is hit when
                    // the final msg.ack() fails after the agent finishes and NATS
                    // redelivers the message. Skip re-running to avoid duplicate
                    // outputs and unnecessary Anthropic API calls.
                    if p.status == crate::promise_store::PromiseStatus::Resolved {
                        info!(
                            promise_id = %pid,
                            iteration = p.iteration,
                            "Promise already Resolved — skipping re-run"
                        );
                        return Ok(String::new());
                    }
                    if p.status == crate::promise_store::PromiseStatus::PermanentFailed {
                        info!(
                            promise_id = %pid,
                            iteration = p.iteration,
                            "Promise permanently failed — skipping re-run (deterministic error, retry cannot succeed)"
                        );
                        return Ok(String::new());
                    }
                    if !p.messages.is_empty() {
                        info!(
                            promise_id = %pid,
                            iteration = p.iteration,
                            "Resuming agent run from checkpoint"
                        );
                        messages = p.messages.clone();
                        recovering = true;
                    }
                    checkpoint = Some((p, rev));
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(error = %e, "Failed to read promise checkpoint — starting fresh")
                }
            }
        }

        // ── Durable promise: pin system prompt ───────────────────────────────
        // When recovering from a checkpoint, use the system prompt that was
        // stored at the start of the original run rather than the freshly-
        // fetched one. This ensures the LLM sees the same context before and
        // after a crash — a changed memory.md between the crash and the
        // recovery would otherwise shift the agent's behaviour mid-run.
        // When not recovering (or when no stored prompt exists), use the
        // caller-supplied value as usual.
        let effective_prompt: Option<String> = if recovering {
            checkpoint
                .as_ref()
                .and_then(|(p, _)| p.system_prompt.clone())
                .or_else(|| system_prompt.map(|s| s.to_string()))
        } else {
            system_prompt.map(|s| s.to_string())
        };

        // Merge caller-supplied tools with MCP tool definitions.
        let mut all_tools: Vec<ToolDef> = tools.to_vec();
        all_tools.extend(self.mcp_tool_defs.iter().cloned());
        if self.elicitation_provider.is_some() {
            all_tools.push(ToolDef {
                name: "ask_user".to_string(),
                description: "Ask the user a question and wait for their response. Use this when you need clarification or additional information to proceed.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "The question to ask the user."
                        }
                    },
                    "required": ["question"]
                }),
                cache_control: None,
            });
        }

        // Mark the last tool with cache_control so Anthropic caches the entire
        // tool definitions block across repeated requests.
        let mut cached_tools: Vec<ToolDef> = all_tools;
        if let Some(last) = cached_tools.last_mut() {
            last.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
        }

        // ── Durable promise: pre-pin system prompt ───────────────────────────
        // The checkpoint path stores system_prompt on the first *successful*
        // LLM response, leaving a window where a crash before that response
        // loses the original prompt. Writing it here — before any LLM call —
        // closes that window so recovery always resumes with the same context
        // even if the very first request never completes.
        //
        // Only do this on a fresh run (not recovering): on recovery the prompt
        // was already stored on the original run's first checkpoint, and we
        // don't want to overwrite it with a potentially stale re-fetch.
        if !recovering
            && let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
            && let Some((ref mut p, ref mut rev)) = checkpoint
            && p.system_prompt.is_none()
            && effective_prompt.is_some()
        {
            p.system_prompt = effective_prompt.clone();
            match tokio::time::timeout(
                NATS_KV_TIMEOUT,
                store.update_promise(&self.tenant_id, pid, p, *rev),
            )
            .await
            {
                Ok(Ok(new_rev)) => *rev = new_rev,
                Ok(Err(e)) => {
                    // CAS conflict — another write landed between our load
                    // and this write. Reload the current revision and retry
                    // once so the prompt is pinned before the first LLM call.
                    warn!(error = %e, promise_id = %pid, "Pre-pin system prompt CAS conflict — reloading revision and retrying");
                    match tokio::time::timeout(
                        NATS_KV_TIMEOUT,
                        store.get_promise(&self.tenant_id, pid),
                    )
                    .await
                    {
                        Ok(Ok(Some((_, new_rev)))) => {
                            *rev = new_rev;
                            match tokio::time::timeout(
                                NATS_KV_TIMEOUT,
                                store.update_promise(&self.tenant_id, pid, p, *rev),
                            )
                            .await
                            {
                                Ok(Ok(new_rev2)) => *rev = new_rev2,
                                Ok(Err(e)) => {
                                    // Keep p.system_prompt set — the first
                                    // successful checkpoint write will persist it.
                                    warn!(error = %e, promise_id = %pid, "Pre-pin system prompt retry failed — system_prompt will be written on first checkpoint");
                                }
                                Err(_) => {
                                    // Timeout: write may have landed. Either way,
                                    // p.system_prompt stays set so the checkpoint
                                    // path persists it on the first successful write.
                                    warn!(promise_id = %pid, "Pre-pin system prompt retry timed out — system_prompt will be written on first checkpoint");
                                }
                            }
                        }
                        _ => {
                            // Keep p.system_prompt set — the checkpoint path will
                            // persist it on the first successful write.
                            warn!(promise_id = %pid, "Could not reload revision for pre-pin retry — system_prompt will be written on first checkpoint");
                        }
                    }
                }
                Err(_) => {
                    // Timeout: write may or may not have landed. Reload and
                    // retry once — if it landed the reload confirms it; if not,
                    // the retry writes it. Keep p.system_prompt set regardless
                    // so the first checkpoint persists it if both paths fail.
                    warn!(promise_id = %pid, "NATS KV timeout pre-pinning system prompt — reloading revision and retrying");
                    match tokio::time::timeout(
                        NATS_KV_TIMEOUT,
                        store.get_promise(&self.tenant_id, pid),
                    )
                    .await
                    {
                        Ok(Ok(Some((reloaded, new_rev)))) => {
                            *rev = new_rev;
                            if reloaded.system_prompt.is_some() {
                                // Write landed before the timeout was observed on
                                // our side — nothing more to do.
                            } else {
                                match tokio::time::timeout(
                                    NATS_KV_TIMEOUT,
                                    store.update_promise(&self.tenant_id, pid, p, *rev),
                                )
                                .await
                                {
                                    Ok(Ok(new_rev2)) => *rev = new_rev2,
                                    Ok(Err(e)) => {
                                        warn!(error = %e, promise_id = %pid,
                                            "Pre-pin system prompt timeout retry failed — system_prompt will be written on first checkpoint");
                                    }
                                    Err(_) => {
                                        warn!(promise_id = %pid,
                                            "Pre-pin system prompt timeout retry also timed out — system_prompt will be written on first checkpoint");
                                    }
                                }
                            }
                        }
                        _ => {
                            // Could not reload revision — keep p.system_prompt
                            // set for the first checkpoint write.
                            warn!(promise_id = %pid,
                                "Could not reload revision after pre-pin timeout — system_prompt will be written on first checkpoint");
                        }
                    }
                }
            }
        }

        // ── Durable promise: ownership heartbeat ────────────────────────────
        // `claimed_at` is refreshed on every checkpoint write, but there is a
        // gap while the LLM call is in flight (up to 5 minutes). If a new
        // process starts during that window, startup recovery uses the stale
        // `claimed_at` to determine whether the run is abandoned. To close the
        // gap, we refresh `claimed_at` in KV every HEARTBEAT_REFRESH_INTERVAL —
        // well under the 10-minute stale threshold — so recovery never fires
        // while the LLM is legitimately working.
        // Track the last time we successfully wrote `claimed_at` to KV
        // (either via a checkpoint or a heartbeat). Initialised to now because
        // the promise was just claimed moments ago at the call site.
        let mut last_heartbeat_at = std::time::Instant::now();

        for iteration in 0..self.max_iterations {
            debug!(iteration, "Agent loop iteration");

            // ── Durable promise: pre-LLM ownership refresh ───────────────────
            // Refresh `claimed_at` in KV before starting the LLM call so that
            // startup recovery on another process cannot false-positively steal
            // this promise while we're waiting for the model's response.
            if last_heartbeat_at.elapsed() >= HEARTBEAT_REFRESH_INTERVAL
                && let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                && let Some((ref mut p, ref mut rev)) = checkpoint
            {
                    p.claimed_at = trogon_automations::now_unix();
                    match tokio::time::timeout(
                        NATS_KV_TIMEOUT,
                        store.update_promise(&self.tenant_id, pid, p, *rev),
                    )
                    .await
                    {
                        Ok(Ok(new_rev)) => {
                            *rev = new_rev;
                            last_heartbeat_at = std::time::Instant::now();
                        }
                        Ok(Err(e)) => {
                            // CAS conflict — reload the current revision and retry
                            // the heartbeat write so claimed_at is actually refreshed.
                            // Without the retry, a single conflict leaves claimed_at
                            // stale for up to HEARTBEAT_REFRESH_INTERVAL, which can
                            // accumulate into a false-positive stale detection if
                            // conflicts occur repeatedly.
                            warn!(error = %e, promise_id = %pid, "Pre-LLM heartbeat CAS conflict — reloading revision and retrying");
                            match tokio::time::timeout(
                                NATS_KV_TIMEOUT,
                                store.get_promise(&self.tenant_id, pid),
                            )
                            .await
                            {
                                Ok(Ok(Some((current, new_rev)))) => {
                                    if current.status
                                        != crate::promise_store::PromiseStatus::Running
                                    {
                                        // Another worker reached a terminal state — stop
                                        // to avoid producing duplicate side effects.
                                        warn!(
                                            promise_id = %pid,
                                            status = ?current.status,
                                            "Promise no longer Running during heartbeat reload — stopping"
                                        );
                                        return Ok(String::new());
                                    }
                                    *rev = new_rev;
                                    p.claimed_at = trogon_automations::now_unix();
                                    match tokio::time::timeout(
                                        NATS_KV_TIMEOUT,
                                        store.update_promise(&self.tenant_id, pid, p, *rev),
                                    )
                                    .await
                                    {
                                        Ok(Ok(new_rev2)) => {
                                            *rev = new_rev2;
                                            last_heartbeat_at = std::time::Instant::now();
                                        }
                                        Ok(Err(e)) => {
                                            warn!(error = %e, promise_id = %pid, "Pre-LLM heartbeat retry failed — claimed_at may become stale")
                                        }
                                        Err(_) => {
                                            warn!(promise_id = %pid, "Pre-LLM heartbeat retry timed out — claimed_at may become stale")
                                        }
                                    }
                                }
                                Ok(Ok(None)) => {
                                    warn!(promise_id = %pid, "Promise vanished during heartbeat reload — stopping");
                                    return Ok(String::new());
                                }
                                Ok(Err(e)) => {
                                    warn!(error = %e, promise_id = %pid, "Could not reload revision after heartbeat CAS conflict — claimed_at may become stale")
                                }
                                Err(_) => {
                                    warn!(promise_id = %pid, "KV timeout reloading revision after heartbeat CAS conflict — claimed_at may become stale")
                                }
                            }
                        }
                        Err(_) => {
                            // Timeout: write may or may not have landed. Reload
                            // to get the current revision so future heartbeats
                            // and the terminal-status write can succeed. Without
                            // this, the revision stays stale and every subsequent
                            // write CAS-conflicts, leaving claimed_at frozen.
                            warn!(promise_id = %pid, "Pre-LLM heartbeat timed out — reloading revision and retrying");
                            match tokio::time::timeout(
                                NATS_KV_TIMEOUT,
                                store.get_promise(&self.tenant_id, pid),
                            )
                            .await
                            {
                                Ok(Ok(Some((current, new_rev)))) => {
                                    if current.status
                                        != crate::promise_store::PromiseStatus::Running
                                    {
                                        warn!(
                                            promise_id = %pid,
                                            status = ?current.status,
                                            "Promise no longer Running during heartbeat timeout-reload — stopping"
                                        );
                                        return Ok(String::new());
                                    }
                                    *rev = new_rev;
                                    p.claimed_at = trogon_automations::now_unix();
                                    match tokio::time::timeout(
                                        NATS_KV_TIMEOUT,
                                        store.update_promise(&self.tenant_id, pid, p, *rev),
                                    )
                                    .await
                                    {
                                        Ok(Ok(new_rev2)) => {
                                            *rev = new_rev2;
                                            last_heartbeat_at = std::time::Instant::now();
                                        }
                                        Ok(Err(e)) => {
                                            warn!(error = %e, promise_id = %pid, "Pre-LLM heartbeat timeout retry failed — claimed_at may become stale")
                                        }
                                        Err(_) => {
                                            warn!(promise_id = %pid, "Pre-LLM heartbeat timeout retry also timed out — claimed_at may become stale")
                                        }
                                    }
                                }
                                Ok(Ok(None)) => {
                                    warn!(promise_id = %pid, "Promise vanished during heartbeat timeout-reload — stopping");
                                    return Ok(String::new());
                                }
                                Ok(Err(e)) => {
                                    warn!(error = %e, promise_id = %pid, "Could not reload revision after heartbeat timeout — claimed_at may become stale")
                                }
                                Err(_) => {
                                    warn!(promise_id = %pid, "KV timeout reloading revision after heartbeat timeout — claimed_at may become stale")
                                }
                            }
                        }
                    }
            }

            // Build the cacheable system block on each iteration (cheap — just wraps a &str).
            let system: Option<Vec<SystemBlock<'_>>> = effective_prompt.as_deref().map(|text| {
                vec![SystemBlock {
                    block_type: "text",
                    text,
                    cache_control: CacheControl::ephemeral(),
                }]
            });

            let anthropic_messages: Vec<AnthropicMessageRef<'_>> =
                messages.iter().map(AnthropicMessageRef).collect();
            let request = AnthropicRequest {
                model: &self.model,
                max_tokens: 4096,
                system,
                tools: &cached_tools,
                messages: anthropic_messages,
            };

            let body =
                serde_json::to_value(&request).expect("AnthropicRequest is always serializable");
            let raw = match self.anthropic_client.complete(body).await {
                Ok(v) => v,
                Err(e) => {
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                        && let Some((ref mut p, ref mut rev)) = checkpoint
                    {
                        // Non-retryable 4xx (400, 401, 403): the request is broken;
                        // retrying will always produce the same error. Mark
                        // PermanentFailed so neither startup recovery nor NATS
                        // redelivery wastes Anthropic credits on a hopeless run.
                        // Exhausted 429 retries and 5xx / transport errors use
                        // Failed — NATS redelivery may succeed.
                        let terminal_status = if matches!(e, AnthropicClientError::Permanent(_)) {
                            crate::promise_store::PromiseStatus::PermanentFailed
                        } else {
                            crate::promise_store::PromiseStatus::Failed
                        };
                        write_promise_terminal(
                            store.as_ref(),
                            &self.tenant_id,
                            pid,
                            p,
                            *rev,
                            terminal_status,
                            "HTTP error",
                        )
                        .await;
                    } else if matches!(e, AnthropicClientError::Permanent(_)) {
                        // checkpoint=None (CAS revision lost) + deterministic 4xx — attempt fresh write
                        // to prevent infinite startup-recovery cycling on a hopeless run.
                        if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
                            try_mark_permanent_failed_fresh(
                                store.as_ref(),
                                &self.tenant_id,
                                pid,
                                "HTTP error (checkpoint lost)",
                            )
                            .await;
                        }
                    }
                    return Err(AgentError::Http(e));
                }
            };
            let response = match serde_json::from_value::<AnthropicResponse>(raw) {
                Ok(r) => r,
                Err(e) => {
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
                        if let Some((ref mut p, ref mut rev)) = checkpoint {
                            // Deterministic: retrying the same response will always fail.
                            write_promise_terminal(
                                store.as_ref(),
                                &self.tenant_id,
                                pid,
                                p,
                                *rev,
                                crate::promise_store::PromiseStatus::PermanentFailed,
                                "deserialization error",
                            )
                            .await;
                        } else {
                            // checkpoint=None — attempt fresh write to prevent cycling.
                            try_mark_permanent_failed_fresh(
                                store.as_ref(),
                                &self.tenant_id,
                                pid,
                                "deserialization error (checkpoint lost)",
                            )
                            .await;
                        }
                    }
                    return Err(AgentError::UnexpectedStopReason(format!(
                        "Deserialization error: {e}"
                    )));
                }
            };

            debug!(stop_reason = %response.stop_reason, "Model response received");

            match response.stop_reason.as_str() {
                "end_turn" => {
                    let text = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    info!(iterations = iteration + 1, "Agent completed");
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
                        if let Some((ref mut p, ref mut rev)) = checkpoint {
                            p.status = crate::promise_store::PromiseStatus::Resolved;
                            let needs_reload = match tokio::time::timeout(
                                NATS_KV_TIMEOUT,
                                store.update_promise(&self.tenant_id, pid, p, *rev),
                            )
                            .await
                            {
                                Ok(Ok(_)) => false,
                                Ok(Err(e)) => {
                                    warn!(error = %e, promise_id = %pid, "CAS conflict marking promise Resolved — reloading revision");
                                    true
                                }
                                Err(_) => {
                                    warn!(promise_id = %pid, "NATS KV write timed out marking promise Resolved — reloading to verify");
                                    true
                                }
                            };
                            if needs_reload {
                                // Reload and retry once — either the write landed (CAS conflict
                                // from a concurrent writer) or we need a fresh revision to retry.
                                match tokio::time::timeout(
                                    NATS_KV_TIMEOUT,
                                    store.get_promise(&self.tenant_id, pid),
                                )
                                .await
                                {
                                    Ok(Ok(Some((current, new_rev))))
                                        if current.status
                                            != crate::promise_store::PromiseStatus::Resolved =>
                                    {
                                        p.status = crate::promise_store::PromiseStatus::Resolved;
                                        *rev = new_rev;
                                        match tokio::time::timeout(
                                            NATS_KV_TIMEOUT,
                                            store.update_promise(&self.tenant_id, pid, p, new_rev),
                                        )
                                        .await
                                        {
                                            Ok(Ok(_)) => {}
                                            Ok(Err(e)) => {
                                                warn!(error = %e, promise_id = %pid, "Failed to mark promise Resolved on retry")
                                            }
                                            Err(_) => {
                                                warn!(promise_id = %pid, "NATS KV write timed out marking promise Resolved on retry")
                                            }
                                        }
                                    }
                                    Ok(Ok(Some(_))) => {} // Already Resolved by another process
                                    Ok(Ok(None)) => {}    // Promise gone from KV
                                    Ok(Err(e)) => {
                                        warn!(error = %e, promise_id = %pid, "Failed to reload promise for Resolved retry")
                                    }
                                    Err(_) => {
                                        warn!(promise_id = %pid, "NATS KV timed out reloading promise for Resolved retry")
                                    }
                                }
                            }
                        } else {
                            // checkpoint=None (CAS revision lost) — attempt a fresh get+write so
                            // the promise doesn't stay Running and trigger startup recovery re-runs.
                            match tokio::time::timeout(
                                NATS_KV_TIMEOUT,
                                store.get_promise(&self.tenant_id, pid),
                            )
                            .await
                            {
                                Ok(Ok(Some((mut p, rev)))) => {
                                    p.status = crate::promise_store::PromiseStatus::Resolved;
                                    match tokio::time::timeout(
                                        NATS_KV_TIMEOUT,
                                        store.update_promise(&self.tenant_id, pid, &p, rev),
                                    )
                                    .await
                                    {
                                        Ok(Ok(_)) => {}
                                        Ok(Err(e)) => {
                                            warn!(error = %e, promise_id = %pid, "Failed to mark promise Resolved (checkpoint lost)")
                                        }
                                        Err(_) => {
                                            warn!(promise_id = %pid, "NATS KV write timed out marking promise Resolved (checkpoint lost)")
                                        }
                                    }
                                }
                                Ok(Ok(None)) => {} // Promise gone from KV — no recovery risk
                                Ok(Err(e)) => {
                                    warn!(error = %e, promise_id = %pid, "Could not fetch promise to mark Resolved (checkpoint lost)")
                                }
                                Err(_) => {
                                    warn!(promise_id = %pid, "NATS KV timeout fetching promise to mark Resolved (checkpoint lost)")
                                }
                            }
                        }
                    }
                    return Ok(text);
                }
                "tool_use" => {
                    // Declared here (not outside the loop) so it resets to false on every
                    // tool turn — each iteration retries the checkpoint in case the payload
                    // has shrunk. The size-guard below re-enables it for this turn only
                    // if trimming still cannot bring the payload under CHECKPOINT_MAX_BYTES.
                    let mut checkpointing_disabled = false;
                    let results = self.execute_tools(&response.content).await;
                    let input_tokens = response.usage.as_ref().map_or(0, |u| u.input_tokens);
                    let mut assistant_msg = Message::assistant(response.content);
                    assistant_msg.usage = response.usage;
                    messages.push(assistant_msg);
                    messages.push(Message::tool_results(results));

                    // ── Live context trim ─────────────────────────────────────
                    // If the previous request consumed a large share of the model's
                    // context window, trim the oldest messages before the next call.
                    // Uses the same summarize_dropped_messages mechanism as the
                    // checkpoint trim, but triggered by token count rather than KV
                    // payload size — and works regardless of whether a promise store
                    // is configured.
                    const CONTEXT_TOKEN_THRESHOLD: u32 = 150_000;
                    const LIVE_TRIM_KEEP: usize = 4;
                    if input_tokens > CONTEXT_TOKEN_THRESHOLD {
                        let keep = (messages.len() / 2)
                            .max(LIVE_TRIM_KEEP)
                            .min(messages.len());
                        let mut drop_count = messages.len() - keep;
                        // Walk back to a user-role boundary — never start a trimmed
                        // history on an assistant message.
                        while drop_count > 0 && messages[drop_count].role != "user" {
                            drop_count -= 1;
                        }
                        if drop_count > 0 {
                            let summary = tokio::time::timeout(
                                std::time::Duration::from_secs(30),
                                summarize_dropped_messages(
                                    &*self.anthropic_client,
                                    &self.model,
                                    &messages[..drop_count],
                                ),
                            )
                            .await
                            .unwrap_or_default();
                            messages = if summary.is_empty() {
                                messages[drop_count..].to_vec()
                            } else {
                                let mut v = summary;
                                v.extend_from_slice(&messages[drop_count..]);
                                v
                            };
                            warn!(
                                input_tokens,
                                kept = messages.len(),
                                "live context trim — history truncated before next Anthropic call"
                            );
                        }
                    }

                    // `recovering` is flipped to `false` inside the successful
                    // checkpoint write arm below, not here. This ensures the tool
                    // result cache is still consulted on the next tool turn if the
                    // checkpoint write fails or is disabled — a crash before the
                    // next checkpoint would land at the same recovery point, and
                    // the cache prevents re-executing already-completed tool calls.

                    // ── Durable promise: checkpoint after tool turn ───────────
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                        && let Some((ref mut p, ref mut rev)) = checkpoint
                    {
                        {
                            // Guard: refuse to write a checkpoint that exceeds
                            // NATS KV's 1MB value limit. Oversized writes are
                            // silently rejected by NATS — the write appears to
                            // succeed but the revision does not advance, so
                            // subsequent CAS writes fail with a conflict error
                            // and the run loses all checkpoint progress.
                            //
                            // Measure size by temporarily staging the new messages
                            // into `p`. If the guard fires, restore the previous
                            // messages and leave all other fields of `p` untouched —
                            // terminal-status writes (Resolved, PermanentFailed) must
                            // not carry stale or oversized history into KV.
                            let prev_messages =
                                std::mem::replace(&mut p.messages, messages.clone());
                            let serialized_len = serde_json::to_vec(p as &_)
                                .map(|v| v.len())
                                .unwrap_or(usize::MAX);
                            if serialized_len > CHECKPOINT_MAX_BYTES {
                                // Before giving up, try trimming the oldest messages so the
                                // payload fits. Halve the history up to three times (keeping
                                // at most the most-recent MIN_TRIM_KEEP messages as a floor)
                                // before permanently disabling checkpointing.
                                //
                                // Once a plain-trim level that fits is found, attempt to
                                // preserve semantic context by summarising the dropped
                                // messages via the LLM and prepending the summary to the
                                // kept slice. If the summary itself is too large or the
                                // call fails, fall back to the plain trim silently.
                                const MIN_TRIM_KEEP: usize = 4;
                                let mut trim_keep: Option<usize> = None;
                                for divisor in [2_usize, 4, 8] {
                                    let keep = (messages.len() / divisor).max(MIN_TRIM_KEEP);
                                    if keep >= messages.len() {
                                        continue; // divisor too small to reduce history
                                    }
                                    p.messages = messages[messages.len() - keep..].to_vec();
                                    let trimmed_len = serde_json::to_vec(p as &_)
                                        .map(|v| v.len())
                                        .unwrap_or(usize::MAX);
                                    if trimmed_len <= CHECKPOINT_MAX_BYTES {
                                        trim_keep = Some(keep);
                                        break;
                                    }
                                }
                                if let Some(keep) = trim_keep {
                                    let drop_count = messages.len() - keep;
                                    // p.messages is already the plain trim (messages[drop_count..]).
                                    // Attempt to enrich with an LLM-generated summary of the dropped
                                    // messages so a crash-recovered run has semantic context.
                                    // Hard cap: summarization is best-effort —
                                    // blocking the checkpoint for up to 5 minutes
                                    // (inherited LLM timeout) is disproportionate.
                                    // 30 s is generous for a brief summary and keeps
                                    // the turn latency predictable. On timeout the
                                    // empty Vec triggers the plain-trim fallback.
                                    let summary_pair = tokio::time::timeout(
                                        std::time::Duration::from_secs(30),
                                        summarize_dropped_messages(
                                            &*self.anthropic_client,
                                            &self.model,
                                            &messages[..drop_count],
                                        ),
                                    )
                                    .await
                                    .unwrap_or_default();
                                    if !summary_pair.is_empty() {
                                        let mut with_summary = summary_pair;
                                        with_summary.extend_from_slice(&messages[drop_count..]);
                                        p.messages = with_summary;
                                        let summary_len = serde_json::to_vec(p as &_)
                                            .map(|v| v.len())
                                            .unwrap_or(usize::MAX);
                                        if summary_len <= CHECKPOINT_MAX_BYTES {
                                            warn!(
                                                promise_id = %pid,
                                                original_messages = messages.len(),
                                                kept_messages = keep,
                                                dropped_messages = drop_count,
                                                bytes = summary_len,
                                                "Checkpoint history trimmed — dropped messages replaced by LLM summary"
                                            );
                                        } else {
                                            // Summary + kept still over limit — revert to plain trim
                                            p.messages = messages[drop_count..].to_vec();
                                            warn!(
                                                promise_id = %pid,
                                                original_messages = messages.len(),
                                                kept_messages = keep,
                                                "Checkpoint history trimmed — oldest messages discarded (LLM summary too large to fit)"
                                            );
                                        }
                                    } else {
                                        warn!(
                                            promise_id = %pid,
                                            original_messages = messages.len(),
                                            kept_messages = keep,
                                            "Checkpoint history trimmed — oldest messages discarded (LLM summarization unavailable)"
                                        );
                                    }
                                    // p.messages is now set to the final slice (summary+kept or plain kept).
                                } else {
                                    p.messages = prev_messages; // restore — p must stay clean
                                    error!(
                                        promise_id = %pid,
                                        size_bytes = serialized_len,
                                        limit_bytes = CHECKPOINT_MAX_BYTES,
                                        checkpoint_degraded = true,
                                        "Checkpoint payload exceeds NATS KV size limit after all trim attempts — \
                                        checkpointing disabled for this run; a crash will resume from the last \
                                        successful checkpoint"
                                    );
                                    checkpointing_disabled = true;
                                    // Persist the degraded flag so operators can observe
                                    // runs that lost durability. Uses prev_messages (the
                                    // last successful checkpoint) which fits by definition.
                                    // Best-effort: if this write fails the run continues
                                    // without the flag — not worth blocking on.
                                    if !p.checkpoint_degraded {
                                        p.checkpoint_degraded = true;
                                        let first_ok = match tokio::time::timeout(
                                            NATS_KV_TIMEOUT,
                                            store.update_promise(&self.tenant_id, pid, p, *rev),
                                        )
                                        .await
                                        {
                                            Ok(Ok(new_rev)) => {
                                                *rev = new_rev;
                                                true
                                            }
                                            Ok(Err(e)) => {
                                                warn!(error = %e, promise_id = %pid,
                                                "checkpoint_degraded write CAS conflict — reloading and retrying");
                                                false
                                            }
                                            Err(_) => {
                                                warn!(promise_id = %pid,
                                                "checkpoint_degraded write timed out — reloading to verify and retry");
                                                false
                                            }
                                        };
                                        if !first_ok {
                                            // Reload revision and retry once. If the first write
                                            // landed despite the error (timeout case), the reload
                                            // will show checkpoint_degraded=true and we skip.
                                            match tokio::time::timeout(
                                                NATS_KV_TIMEOUT,
                                                store.get_promise(&self.tenant_id, pid),
                                            )
                                            .await
                                            {
                                                Ok(Ok(Some((ref current, fresh_rev))))
                                                    if !current.checkpoint_degraded =>
                                                {
                                                    // First write did not land — retry with fresh rev.
                                                    match tokio::time::timeout(
                                                        NATS_KV_TIMEOUT,
                                                        store.update_promise(
                                                            &self.tenant_id,
                                                            pid,
                                                            p,
                                                            fresh_rev,
                                                        ),
                                                    )
                                                    .await
                                                    {
                                                        Ok(Ok(new_rev)) => *rev = new_rev,
                                                        Ok(Err(e)) => warn!(
                                                            error = %e, promise_id = %pid,
                                                            "Could not persist checkpoint_degraded flag after retry — flag visible in-memory only"
                                                        ),
                                                        Err(_) => warn!(
                                                            promise_id = %pid,
                                                            "NATS KV timeout persisting checkpoint_degraded flag on retry — flag visible in-memory only"
                                                        ),
                                                    }
                                                }
                                                Ok(Ok(Some((_, fresh_rev)))) => {
                                                    // First write landed despite error response.
                                                    *rev = fresh_rev;
                                                }
                                                _ => warn!(
                                                    promise_id = %pid,
                                                    "Could not reload revision for checkpoint_degraded retry — flag visible in-memory only"
                                                ),
                                            }
                                        }
                                    }
                                }
                            }
                            // Size fits (as-is or after trimming) — commit all fields to p.
                            if !checkpointing_disabled {
                                p.iteration = iteration + 1;
                                p.claimed_at = trogon_automations::now_unix(); // refresh ownership lease
                                // Persist system prompt on the first checkpoint so that
                                // crash recovery can restore the original LLM context.
                                if p.system_prompt.is_none() {
                                    p.system_prompt = effective_prompt.clone();
                                }

                                match tokio::time::timeout(
                                    NATS_KV_TIMEOUT,
                                    store.update_promise(&self.tenant_id, pid, p, *rev),
                                )
                                .await
                                {
                                    Ok(Ok(new_rev)) => {
                                        *rev = new_rev;
                                        // Checkpoint just wrote a fresh claimed_at — reset the
                                        // heartbeat timer so we don't immediately re-write.
                                        last_heartbeat_at = std::time::Instant::now();
                                    }
                                    Ok(Err(e)) => {
                                        // CAS conflict: another process wrote to this promise
                                        // between our last read and now. Reload the current
                                        // revision so future checkpoints can succeed.
                                        // Without this reload, every subsequent checkpoint in
                                        // this run fails silently with a stale revision.
                                        warn!(error = %e, "Promise checkpoint CAS conflict — reloading revision");
                                        match tokio::time::timeout(
                                            NATS_KV_TIMEOUT,
                                            store.get_promise(&self.tenant_id, pid),
                                        )
                                        .await
                                        {
                                            Ok(Ok(Some((current, new_rev)))) => {
                                                // If another worker reached a terminal state
                                                // between our tool turn and the CAS write,
                                                // stop immediately — continuing would produce
                                                // duplicate side effects (extra PR comments,
                                                // Slack messages, etc.) for a run that already
                                                // completed. Return an empty string so the
                                                // caller's ack path still runs cleanly.
                                                if current.status
                                                    != crate::promise_store::PromiseStatus::Running
                                                {
                                                    warn!(
                                                        promise_id = %pid,
                                                        status = ?current.status,
                                                        "Promise was resolved/failed by another worker during CAS reload — stopping this run to avoid duplicate outputs"
                                                    );
                                                    return Ok(String::new());
                                                }
                                                *rev = new_rev;
                                            }
                                            Ok(Ok(None)) => {
                                                error!(
                                                    promise_id = %pid,
                                                    "Promise disappeared during CAS reload — checkpointing disabled for this run"
                                                );
                                                // The promise is gone from KV — every subsequent
                                                // write would fail too, and the misleading
                                                // "CAS conflict" log would repeat each turn.
                                                // Terminal-status writes are also skipped; this
                                                // is acceptable since the promise is already gone.
                                                checkpoint = None;
                                            }
                                            Ok(Err(e)) => {
                                                // NATS returned an error. Keep the current revision
                                                // so terminal-status writes (Resolved, PermanentFailed)
                                                // can still succeed. The next checkpoint write will
                                                // either land with the old rev (if it's still valid)
                                                // or CAS-conflict again and trigger another reload.
                                                error!(
                                                    error = %e,
                                                    promise_id = %pid,
                                                    "CAS reload failed — keeping current revision; will retry on next checkpoint"
                                                );
                                            }
                                            Err(_) => {
                                                // Reload timed out. Same reasoning as the error arm:
                                                // keep current revision to preserve terminal-status
                                                // write capability. A crashed process that recovers
                                                // from the last successful checkpoint is strictly
                                                // better than a stuck-Running promise.
                                                warn!(
                                                    promise_id = %pid,
                                                    "NATS KV get_promise timed out during CAS reload — keeping current revision; will retry on next checkpoint"
                                                );
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        // The write may or may not have landed on the server.
                                        // Reload the current revision: if the write succeeded,
                                        // we pick up the new rev and checkpointing continues
                                        // normally on the next turn; if it failed, we keep the
                                        // old rev and retry. Without reloading, the revision
                                        // stays stale, subsequent CAS writes all fail with
                                        // conflict errors, checkpoint eventually becomes None,
                                        // and terminal-status writes (Resolved, PermanentFailed)
                                        // are blocked for the rest of the run.
                                        warn!(promise_id = %pid, "NATS KV update_promise timed out — reloading revision to restore checkpoint");
                                        match tokio::time::timeout(
                                            NATS_KV_TIMEOUT,
                                            store.get_promise(&self.tenant_id, pid),
                                        )
                                        .await
                                        {
                                            Ok(Ok(Some((current, new_rev)))) => {
                                                if current.status
                                                    != crate::promise_store::PromiseStatus::Running
                                                {
                                                    warn!(
                                                        promise_id = %pid,
                                                        status = ?current.status,
                                                        "Promise reached terminal state during timeout-reload — stopping this run to avoid duplicate outputs"
                                                    );
                                                    return Ok(String::new());
                                                }
                                                *rev = new_rev;
                                            }
                                            Ok(Ok(None)) => {
                                                // Promise genuinely gone from KV — no point writing.
                                                error!(promise_id = %pid, "Promise disappeared during timeout-reload — checkpointing disabled for this run");
                                                checkpoint = None;
                                            }
                                            Ok(Err(e)) => {
                                                // NATS error. Keep the current revision so
                                                // terminal-status writes can still succeed.
                                                // Worst case: next checkpoint write CAS-conflicts
                                                // and triggers another reload attempt.
                                                error!(error = %e, promise_id = %pid, "Timeout-reload get_promise failed — keeping current revision; will retry on next checkpoint");
                                            }
                                            Err(_) => {
                                                // The reload itself timed out. Keep the current
                                                // revision — a stale rev that retries is better
                                                // than a stuck-Running promise.
                                                warn!(promise_id = %pid, "NATS KV get_promise timed out during timeout-reload — keeping current revision; will retry on next checkpoint");
                                            }
                                        }
                                    }
                                }
                            } // if !checkpointing_disabled: size fits (as-is or trimmed)
                        } // checkpoint attempt block
                    }
                }
                "max_tokens" => {
                    // Deterministic: the conversation has exceeded the model's
                    // context window. The same history always hits max_tokens on
                    // retry — mark PermanentFailed to stop cycling.
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
                        if let Some((ref mut p, ref mut rev)) = checkpoint {
                            write_promise_terminal(
                                store.as_ref(),
                                &self.tenant_id,
                                pid,
                                p,
                                *rev,
                                crate::promise_store::PromiseStatus::PermanentFailed,
                                "max_tokens",
                            )
                            .await;
                        } else {
                            try_mark_permanent_failed_fresh(
                                store.as_ref(),
                                &self.tenant_id,
                                pid,
                                "max_tokens (checkpoint lost)",
                            )
                            .await;
                        }
                    }
                    return Err(AgentError::UnexpectedStopReason("max_tokens".to_string()));
                }
                other => {
                    // Unknown stop_reason: treat as transient (`Failed`) so
                    // NATS redelivery can retry. A future Anthropic API
                    // addition may use a new stop_reason that is not
                    // deterministic — permanently failing would discard the
                    // run with no retry opportunity.
                    // checkpoint=None: no CAS revision; returning Err is
                    // enough — NATS will redeliver and retry from the last
                    // valid checkpoint without any explicit KV write here.
                    let reason = other.to_string();
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                        && let Some((ref mut p, ref mut rev)) = checkpoint
                    {
                        write_promise_terminal(
                            store.as_ref(),
                            &self.tenant_id,
                            pid,
                            p,
                            *rev,
                            crate::promise_store::PromiseStatus::Failed,
                            "unexpected stop reason",
                        )
                        .await;
                    }
                    return Err(AgentError::UnexpectedStopReason(reason));
                }
            }
        }

        warn!(max = self.max_iterations, "Agent reached max iterations");
        if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
            if let Some((ref mut p, ref mut rev)) = checkpoint {
                // Deterministic: same messages + same max_iterations = same outcome.
                write_promise_terminal(
                    store.as_ref(),
                    &self.tenant_id,
                    pid,
                    p,
                    *rev,
                    crate::promise_store::PromiseStatus::PermanentFailed,
                    "max iterations",
                )
                .await;
            } else {
                // checkpoint=None — attempt fresh write to prevent cycling.
                try_mark_permanent_failed_fresh(
                    store.as_ref(),
                    &self.tenant_id,
                    pid,
                    "max iterations (checkpoint lost)",
                )
                .await;
            }
        }
        Err(AgentError::MaxIterationsReached)
    }

    /// Like [`run`] but also returns the full updated message history.
    ///
    /// Used by the interactive chat API to persist conversation across turns.
    /// `initial_messages` should contain the prior history; the returned
    /// `Vec<Message>` is that history extended with the new user turn, all
    /// intermediate tool exchanges, and the final assistant turn.
    pub async fn run_chat(
        &self,
        initial_messages: Vec<Message>,
        tools: &[ToolDef],
        system_prompt: Option<&str>,
    ) -> Result<(String, Vec<Message>), AgentError> {
        let mut messages = initial_messages;

        let mut all_tools: Vec<ToolDef> = tools.to_vec();
        all_tools.extend(self.mcp_tool_defs.iter().cloned());
        if self.elicitation_provider.is_some() {
            all_tools.push(ToolDef {
                name: "ask_user".to_string(),
                description: "Ask the user a question and wait for their response. Use this when you need clarification or additional information to proceed.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "The question to ask the user."
                        }
                    },
                    "required": ["question"]
                }),
                cache_control: None,
            });
        }
        let mut cached_tools: Vec<ToolDef> = all_tools;
        if let Some(last) = cached_tools.last_mut() {
            last.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
        }

        for iteration in 0..self.max_iterations {
            debug!(iteration, "Chat loop iteration");

            let system: Option<Vec<SystemBlock<'_>>> = system_prompt.map(|text| {
                vec![SystemBlock {
                    block_type: "text",
                    text,
                    cache_control: CacheControl::ephemeral(),
                }]
            });

            let anthropic_messages: Vec<AnthropicMessageRef<'_>> =
                messages.iter().map(AnthropicMessageRef).collect();
            let request = AnthropicRequest {
                model: &self.model,
                max_tokens: 4096,
                system,
                tools: &cached_tools,
                messages: anthropic_messages,
            };

            let body =
                serde_json::to_value(&request).expect("AnthropicRequest is always serializable");
            let raw = self
                .anthropic_client
                .complete(body)
                .await
                .map_err(AgentError::Http)?;
            let response = serde_json::from_value::<AnthropicResponse>(raw).map_err(|e| {
                AgentError::UnexpectedStopReason(format!("Deserialization error: {e}"))
            })?;

            match response.stop_reason.as_str() {
                "end_turn" => {
                    let text = response
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let mut assistant_msg = Message::assistant(response.content);
                    assistant_msg.usage = response.usage;
                    messages.push(assistant_msg);
                    info!(iterations = iteration + 1, "Chat completed");
                    return Ok((text, messages));
                }
                "tool_use" => {
                    let results = self.execute_tools(&response.content).await;
                    let mut assistant_msg = Message::assistant(response.content);
                    assistant_msg.usage = response.usage;
                    messages.push(assistant_msg);
                    messages.push(Message::tool_results(results));
                }
                other => {
                    return Err(AgentError::UnexpectedStopReason(other.to_string()));
                }
            }
        }

        warn!(max = self.max_iterations, "Chat reached max iterations");
        Err(AgentError::MaxIterationsReached)
    }

    /// Execute tool calls from an Anthropic response.
    ///
    /// Executes the tool calls in `content`, consulting the KV tool-result
    /// cache whenever a `promise_id` is set on this agent.
    ///
    /// The cache is keyed on `(tool_name, canonical_json(input))` rather than
    /// the ephemeral `tool_use_id`. This means:
    /// - On crash recovery the LLM re-generates a fresh `tool_use_id` for the
    ///   same call, but the content-based key is identical → cache hit → no
    ///   re-execution of the side effect.
    /// - Within a single run, the same (name, input) called in a later turn
    ///   returns the earlier result from cache, preventing intra-run duplicates
    ///   (e.g. posting the same PR comment twice). For pure-read tools called
    ///   repeatedly with the same input the cached result may be one or more
    ///   turns stale; this is an accepted trade-off given that side-effecting
    ///   tools greatly outnumber read tools in practice.
    ///
    /// When `promise_id` is `None` (non-durable or chat runs) the cache is
    /// never consulted and every tool executes fresh.
    async fn execute_tools(&self, content: &[ContentBlock]) -> Vec<ToolResult> {
        let mut results = Vec::new();

        for block in content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, "Executing tool");

                // ── Durable promise: replay cached result ────────────────────
                // Consult the KV cache whenever a promise_id is set. The cache
                // is populated by `put_tool_result` after every tool execution,
                // so a hit means the tool ran successfully in a previous attempt
                // (either an earlier turn of this run or a prior crashed run).
                // Replaying the cached result avoids re-executing side effects
                // and works correctly across process restarts because the key is
                // content-based, not tied to the ephemeral tool_use_id.
                if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
                    let ck = tool_cache_key(name, input);
                    let cache_result = match tokio::time::timeout(
                        NATS_KV_TIMEOUT,
                        store.get_tool_result(&self.tenant_id, pid, &ck),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            // Cannot determine if this tool already executed.
                            // Re-executing risks duplicate side effects on write
                            // tools (Slack messages, Linear comments, GitHub
                            // comments). Return an error result so the LLM can
                            // stop the run; the promise is retried from the last
                            // checkpoint on recovery.
                            error!(
                                tool = %name,
                                promise_id = %pid,
                                "NATS KV get_tool_result timed out — skipping tool execution to avoid duplicate side effects"
                            );
                            results.push(ToolResult {
                                tool_use_id: id.clone(),
                                content: "Tool cache lookup failed (NATS timeout). \
                                    The previous execution of this tool may have already \
                                    produced side effects. Stop this run immediately \
                                    and do not call any further tools."
                                    .to_string(),
                            });
                            continue;
                        }
                    };
                    match cache_result {
                        Ok(Some(cached)) => {
                            info!(tool = %name, tool_use_id = %id, "Replaying cached tool result");
                            results.push(ToolResult {
                                tool_use_id: id.clone(),
                                content: cached,
                            });
                            continue;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            // Same risk as timeout: cannot tell if tool already ran.
                            error!(
                                error = %e,
                                tool = %name,
                                promise_id = %pid,
                                "Failed to read tool result cache — skipping tool execution to avoid duplicate side effects"
                            );
                            results.push(ToolResult {
                                tool_use_id: id.clone(),
                                content: format!(
                                    "Tool cache lookup failed: {e}. \
                                    The previous execution may have produced side effects. \
                                    Stop this run immediately and do not call any further tools."
                                ),
                            });
                            continue;
                        }
                    }
                }

                // Build effective input — inject idempotency key for write tools when available.
                let call_input = if let Some(pid) = &self.promise_id {
                    let mut v = input.clone();
                    if let Some(map) = v.as_object_mut() {
                        map.insert(
                            "_idempotency_key".to_string(),
                            // Use the content-based cache key (SHA-256 of name+input)
                            // rather than the ephemeral tool_use_id so the key is
                            // stable across retries: on recovery the LLM regenerates
                            // a fresh tool_use_id for the same call, but the content
                            // hash is identical — meaning Slack/Linear dedup checks
                            // will match the original key and block duplicate effects.
                            serde_json::Value::String(format!(
                                "{pid}.{}",
                                tool_cache_key(name, input)
                            )),
                        );
                    }
                    v
                } else {
                    input.clone()
                };

                // Built-in `ask_user` tool — handled directly, bypasses permission gate.
                if name == "ask_user" {
                    let question = input
                        .get("question")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let answer = match &self.elicitation_provider {
                        Some(provider) => match provider.elicit(question).await {
                            Some(a) => a,
                            None => "The user declined or cancelled the request.".to_string(),
                        },
                        None => "ask_user tool is not available in this context.".to_string(),
                    };
                    results.push(ToolResult {
                        tool_use_id: id.clone(),
                        content: answer,
                    });
                    continue;
                }

                // Permission gate — fires before MCP and built-in tool dispatch.
                if let Some(checker) = &self.permission_checker {
                    if !checker.check(id, name, input).await {
                        results.push(ToolResult {
                            tool_use_id: id.clone(),
                            content: format!("Permission denied: user refused to run tool `{name}`"),
                        });
                        continue;
                    }
                }

                // Check MCP dispatch first, then fall back to built-in tools.
                //
                // MCP servers receive the original `input` — not `call_input`.
                // `_idempotency_key` is an internal contract between the agent
                // loop and the built-in handlers (GitHub, Slack, Linear). MCP
                // servers are external code that do not consume the field, and a
                // server that declares its tool schema with
                // `additionalProperties: false` would reject the call with an
                // unknown-field error. Built-in tools receive `call_input` which
                // includes the key so their dedup logic fires correctly.
                //
                // MCP tool results ARE transparently cached in AGENT_TOOL_RESULTS
                // (keyed on `(name, input)` — same as built-in tools) by the
                // `put_tool_result` block below. On recovery the cache lookup above
                // fires before this dispatch block and skips re-execution for both
                // MCP and built-in tools. MCP servers themselves do not need to
                // implement idempotency for crash-recovery correctness.
                //
                // Both paths are wrapped with `TOOL_EXECUTION_TIMEOUT` so a
                // hung external API cannot block the run indefinitely while the
                // heartbeat keeps the NATS message alive.
                // `timed_out` is set when the tool execution hit the deadline
                // before returning. It controls caching below: a timeout result
                // must NOT be persisted because we cannot know whether the
                // server processed the request before we stopped waiting.
                // Caching it would permanently prevent recovery from retrying
                // the call — the LLM would see "timed out" from the cache and
                // have no way to issue the call again. Leaving the cache empty
                // lets recovery re-execute the tool; the per-tool idempotency
                // mechanisms (comment marker scan, Linear header, Slack history
                // check) safely deduplicate if the server did process it.
                let (output, timed_out) = if let Some((_, original, client)) = self
                    .mcp_dispatch
                    .iter()
                    .find(|(prefixed, _, _)| prefixed == name)
                {
                    match tokio::time::timeout(
                        TOOL_EXECUTION_TIMEOUT,
                        client.call_tool(original, input),
                    )
                    .await
                    {
                        Ok(Ok(out)) => (out, false),
                        Ok(Err(e)) => (format!("Tool error: {e}"), false),
                        Err(_) => {
                            warn!(
                                tool = %name,
                                timeout_secs = TOOL_EXECUTION_TIMEOUT.as_secs(),
                                "Tool execution timed out"
                            );
                            (
                                format!(
                                    "Tool error: execution timed out after {}s",
                                    TOOL_EXECUTION_TIMEOUT.as_secs()
                                ),
                                true,
                            )
                        }
                    }
                } else {
                    match tokio::time::timeout(
                        TOOL_EXECUTION_TIMEOUT,
                        self.tool_dispatcher.dispatch(name, &call_input),
                    )
                    .await
                    {
                        Ok(output) => (output, false),
                        Err(_) => {
                            warn!(
                                tool = %name,
                                timeout_secs = TOOL_EXECUTION_TIMEOUT.as_secs(),
                                "Tool execution timed out"
                            );
                            (
                                format!(
                                    "Tool error: execution timed out after {}s",
                                    TOOL_EXECUTION_TIMEOUT.as_secs()
                                ),
                                true,
                            )
                        }
                    }
                };

                // ── Durable promise: persist tool result ─────────────────────
                // Store the result keyed by (tool_name, input) so that on crash
                // recovery — when Anthropic re-generates a fresh tool_use_id for
                // the same call — the cache still matches and the tool is not
                // re-executed. Skipped for timeout results (see `timed_out`).
                if !timed_out
                    && let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                {
                    let ck = tool_cache_key(name, input);
                    match tokio::time::timeout(
                        NATS_KV_TIMEOUT,
                        store.put_tool_result(&self.tenant_id, pid, &ck, &output),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            error!(error = %e, "Failed to cache tool result — if process crashes before next checkpoint, this tool will be re-executed on recovery");
                        }
                        Err(_) => {
                            warn!(
                                tool = %name,
                                promise_id = %pid,
                                "NATS KV put_tool_result timed out — retrying once"
                            );
                            match tokio::time::timeout(
                                NATS_KV_TIMEOUT,
                                store.put_tool_result(&self.tenant_id, pid, &ck, &output),
                            )
                            .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => {
                                    error!(error = %e, tool = %name, promise_id = %pid, "Failed to cache tool result after retry — tool will be re-executed on recovery if process crashes");
                                }
                                Err(_) => {
                                    warn!(
                                        tool = %name,
                                        promise_id = %pid,
                                        "NATS KV put_tool_result timed out on retry — tool will be re-executed on recovery if process crashes"
                                    );
                                }
                            }
                        }
                    }
                } // end if !timed_out

                results.push(ToolResult {
                    tool_use_id: id.clone(),
                    content: output,
                });
            }
        }

        results
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn message_user_text_has_correct_role_and_content() {
        let msg = Message::user_text("hello");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.len(), 1);
        assert!(matches!(&msg.content[0], ContentBlock::Text { text } if text == "hello"));
    }

    #[test]
    fn message_tool_results_wraps_correctly() {
        let results = vec![ToolResult {
            tool_use_id: "id1".to_string(),
            content: "output".to_string(),
        }];
        let msg = Message::tool_results(results);
        assert_eq!(msg.role, "user");
        assert!(matches!(
            &msg.content[0],
            ContentBlock::ToolResult { tool_use_id, content }
            if tool_use_id == "id1" && content == "output"
        ));
    }

    #[test]
    fn agent_error_display() {
        assert!(
            AgentError::MaxIterationsReached
                .to_string()
                .contains("max iterations")
        );
        assert!(
            AgentError::UnexpectedStopReason("pause".to_string())
                .to_string()
                .contains("pause")
        );
    }

    #[test]
    fn agent_error_source_for_http_variant() {
        let agent_err = AgentError::Http(AnthropicClientError::Transient("simulated error".into()));
        assert!(std::error::Error::source(&agent_err).is_some());
    }

    #[test]
    fn agent_error_source_none_for_non_http() {
        assert!(std::error::Error::source(&AgentError::MaxIterationsReached).is_none());
    }

    /// When `system_prompt` is `Some`, the serialized request body contains a
    /// `"system"` array with a single block whose `"type"` is `"text"` and
    /// `"cache_control"` is `{"type":"ephemeral"}`.
    #[test]
    fn anthropic_request_serializes_system_block_when_present() {
        use crate::tools::tool_def;
        use serde_json::json;

        let tools = vec![tool_def("t", "d", json!({"type": "object"}))];
        let text = "You are helpful.";
        let system: Option<Vec<SystemBlock<'_>>> = Some(vec![SystemBlock {
            block_type: "text",
            text,
            cache_control: CacheControl::ephemeral(),
        }]);
        let req = AnthropicRequest {
            model: "test-model",
            max_tokens: 1024,
            system,
            tools: &tools,
            messages: vec![],
        };
        let body = serde_json::to_value(&req).unwrap();

        let sys_arr = body["system"]
            .as_array()
            .expect("system should be an array");
        assert_eq!(sys_arr.len(), 1);
        assert_eq!(sys_arr[0]["type"], "text");
        assert_eq!(sys_arr[0]["text"], text);
        assert_eq!(sys_arr[0]["cache_control"]["type"], "ephemeral");
    }

    /// `AgentLoop::run` marks the last tool with `cache_control: ephemeral` so
    /// Anthropic caches the entire tool definitions block across iterations.
    /// Only the *last* tool gets the marker — earlier ones must not have it.
    #[test]
    fn run_marks_last_tool_with_cache_control() {
        use crate::tools::tool_def;
        use serde_json::json;

        // Simulate what AgentLoop::run does with cached_tools.
        let mut cached_tools = [
            tool_def("tool_a", "first tool", json!({"type": "object"})),
            tool_def("tool_b", "second tool", json!({"type": "object"})),
            tool_def("tool_c", "last tool", json!({"type": "object"})),
        ];
        if let Some(last) = cached_tools.last_mut() {
            last.cache_control = Some(json!({"type": "ephemeral"}));
        }

        // Only the last tool should have cache_control.
        assert!(
            cached_tools[0].cache_control.is_none(),
            "first tool must not have cache_control"
        );
        assert!(
            cached_tools[1].cache_control.is_none(),
            "middle tool must not have cache_control"
        );
        assert_eq!(
            cached_tools[2].cache_control,
            Some(json!({"type": "ephemeral"})),
            "last tool must have cache_control: ephemeral"
        );
    }

    /// When there is only one tool it still gets `cache_control: ephemeral`.
    #[test]
    fn run_marks_single_tool_with_cache_control() {
        use crate::tools::tool_def;
        use serde_json::json;

        let mut cached_tools = [tool_def("only", "only tool", json!({"type": "object"}))];
        if let Some(last) = cached_tools.last_mut() {
            last.cache_control = Some(json!({"type": "ephemeral"}));
        }

        assert_eq!(
            cached_tools[0].cache_control,
            Some(json!({"type": "ephemeral"}))
        );
    }

    /// When the tool list is empty no panic occurs and no cache_control is set.
    #[test]
    fn run_empty_tool_list_does_not_panic() {
        use serde_json::json;

        let mut cached_tools: Vec<crate::tools::ToolDef> = vec![];
        if let Some(last) = cached_tools.last_mut() {
            last.cache_control = Some(json!({"type": "ephemeral"}));
        }
        assert!(cached_tools.is_empty());
    }

    /// When `system_prompt` is `None`, the `"system"` key is absent from the
    /// serialized body (thanks to `skip_serializing_if = "Option::is_none"`).
    #[test]
    fn anthropic_request_omits_system_block_when_none() {
        use crate::tools::tool_def;
        use serde_json::json;

        let tools = vec![tool_def("t", "d", json!({"type": "object"}))];
        let req = AnthropicRequest::<'_> {
            model: "test-model",
            max_tokens: 1024,
            system: None,
            tools: &tools,
            messages: vec![],
        };
        let body = serde_json::to_value(&req).unwrap();
        assert!(
            body.get("system").is_none(),
            "system key should be absent when None"
        );
    }

    // ── is_flag_enabled ───────────────────────────────────────────────────────

    fn make_test_agent(split_client: Option<trogon_splitio::SplitClient>) -> AgentLoop {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::{AlwaysOnFlagClient, SplitFlagClient};
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));

        let flag_client: Arc<dyn crate::flag_client::FeatureFlagClient> = match split_client {
            Some(c) => Arc::new(SplitFlagClient::new(c)),
            None => Arc::new(AlwaysOnFlagClient),
        };

        AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![])),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client,
            tenant_id: "test-tenant".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        }
    }

    /// When `split_client` is `None`, `is_flag_enabled` returns `true` for any flag
    /// (fail-open: no Split.io configured → all handlers run).
    #[tokio::test]
    async fn is_flag_enabled_returns_true_when_no_split_client() {
        use crate::flags::AgentFlag;
        let agent = make_test_agent(None);
        assert!(agent.is_flag_enabled(&AgentFlag::PrReviewEnabled).await);
        assert!(agent.is_flag_enabled(&AgentFlag::MemoryEnabled).await);
        assert!(agent.is_flag_enabled(&AgentFlag::AlertHandlerEnabled).await);
    }

    /// When `split_client` is configured and the mock evaluator returns `"on"`,
    /// `is_flag_enabled` returns `true`.
    #[tokio::test]
    async fn is_flag_enabled_returns_true_when_evaluator_says_on() {
        use crate::flags::AgentFlag;
        use trogon_splitio::mock::MockEvaluator;

        let mock = MockEvaluator::new().with_flag("agent_pr_review_enabled", "on");
        let (addr, _h) = mock.serve().await;
        let client = trogon_splitio::SplitClient::new(trogon_splitio::SplitConfig {
            evaluator_url: format!("http://{addr}"),
            auth_token: "tok".to_string(),
        });
        let agent = make_test_agent(Some(client));
        assert!(agent.is_flag_enabled(&AgentFlag::PrReviewEnabled).await);
    }

    /// When `split_client` is configured and the mock evaluator returns `"off"`,
    /// `is_flag_enabled` returns `false`.
    #[tokio::test]
    async fn is_flag_enabled_returns_false_when_evaluator_says_off() {
        use crate::flags::AgentFlag;
        use trogon_splitio::mock::MockEvaluator;

        let mock = MockEvaluator::new().with_flag("agent_pr_review_enabled", "off");
        let (addr, _h) = mock.serve().await;
        let client = trogon_splitio::SplitClient::new(trogon_splitio::SplitConfig {
            evaluator_url: format!("http://{addr}"),
            auth_token: "tok".to_string(),
        });
        let agent = make_test_agent(Some(client));
        assert!(!agent.is_flag_enabled(&AgentFlag::PrReviewEnabled).await);
    }

    /// When the evaluator is unreachable, `is_flag_enabled` returns `false`
    /// (SplitClient returns "control" on error → not "on" → false).
    #[tokio::test]
    async fn is_flag_enabled_returns_false_when_evaluator_unreachable() {
        use crate::flags::AgentFlag;
        let client = trogon_splitio::SplitClient::new(trogon_splitio::SplitConfig {
            evaluator_url: "http://127.0.0.1:1".to_string(),
            auth_token: "tok".to_string(),
        });
        let agent = make_test_agent(Some(client));
        assert!(!agent.is_flag_enabled(&AgentFlag::MemoryEnabled).await);
    }

    /// `tenant_id` is used as the Split.io user key.
    #[tokio::test]
    async fn is_flag_enabled_uses_tenant_id_as_key() {
        use crate::flags::AgentFlag;
        use trogon_splitio::mock::MockEvaluator;

        // Mock returns "on" for any key (mock doesn't do per-user targeting).
        let mock = MockEvaluator::new().with_flag("agent_memory_enabled", "on");
        let (addr, _h) = mock.serve().await;
        let client = trogon_splitio::SplitClient::new(trogon_splitio::SplitConfig {
            evaluator_url: format!("http://{addr}"),
            auth_token: "tok".to_string(),
        });
        let mut agent = make_test_agent(Some(client));
        agent.tenant_id = "acme-corp".to_string();
        assert!(agent.is_flag_enabled(&AgentFlag::MemoryEnabled).await);
    }

    // ── Durable promise recovery integration tests ────────────────────────────

    /// Build an [`AgentLoop`] wired with the given mocks and a live promise store.
    fn make_durable_agent(
        anthropic: Arc<dyn AnthropicClient>,
        dispatcher: Arc<dyn crate::tools::ToolDispatcher>,
        promise_store: Arc<dyn crate::promise_store::PromiseRepository>,
        promise_id: &str,
    ) -> AgentLoop {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::ToolContext;

        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: dispatcher,
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(promise_store),
            promise_id: Some(promise_id.to_string()),
            permission_checker: None,
            elicitation_provider: None,
        }
    }

    /// Build a minimal [`AgentPromise`] for use in tests.
    fn make_test_promise(id: &str) -> crate::promise_store::AgentPromise {
        crate::promise_store::AgentPromise {
            id: id.to_string(),
            tenant_id: "acme".to_string(),
            automation_id: String::new(),
            status: crate::promise_store::PromiseStatus::Running,
            messages: vec![],
            iteration: 0,
            worker_id: "test-worker".to_string(),
            claimed_at: 0,
            trigger: serde_json::Value::Null,
            nats_subject: "test.subject".to_string(),
            system_prompt: None,
            recovery_count: 0,
            checkpoint_degraded: false,
            failure_reason: None,
        }
    }

    /// A promise that is already `Resolved` must not trigger any Anthropic call.
    ///
    /// This covers the case where the agent completed successfully but the
    /// `msg.ack()` failed and NATS redelivered the trigger message. The agent
    /// must detect the `Resolved` status and return immediately without
    /// re-running the LLM or re-executing any tools.
    #[tokio::test]
    async fn resolved_promise_skips_rerun() {
        use crate::promise_store::PromiseStatus;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        let mut p = make_test_promise("p1");
        p.status = PromiseStatus::Resolved;
        store.insert_promise(p);

        // Empty response queue — panics if Anthropic is called at all.
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![]));
        let dispatcher = Arc::new(MockToolDispatcher::new("should not run"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn crate::promise_store::PromiseRepository>,
            "p1",
        );

        let result = agent
            .run(vec![Message::user_text("do stuff")], &[], None)
            .await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "",
            "Resolved promise must return empty string immediately"
        );
    }

    /// After a crash mid-run, tool results already persisted to KV must be
    /// replayed from cache on recovery rather than re-executing the tool.
    ///
    /// Scenario:
    ///   1. Checkpoint exists with non-empty messages (recovering = true).
    ///   2. A tool result for the upcoming LLM turn is already in KV cache
    ///      (the tool ran before the crash but the checkpoint wasn't written).
    ///   3. On recovery, `execute_tools` must replay the cached result without
    ///      calling the dispatcher.
    #[tokio::test]
    async fn recovery_replays_tool_from_cache() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::CountingMockToolDispatcher;
        use mock::SequencedMockAnthropicClient;
        use std::sync::atomic::Ordering;

        let store = Arc::new(MockPromiseStore::new());

        // Non-empty checkpoint messages → `recovering = true` inside `run()`.
        let mut p = make_test_promise("p1");
        p.messages = vec![Message::user_text("start")];
        store.insert_promise(p);

        // Pre-populate the tool result cache (simulates tool ran before crash).
        let input = serde_json::json!({"k": "v"});
        let ck = super::tool_cache_key("my_tool", &input);
        store
            .put_tool_result("acme", "p1", &ck, "cached output")
            .await
            .unwrap();

        // Anthropic: first call returns tool_use, second returns end_turn.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {"k": "v"}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "all done"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let (dispatcher, call_count) = CountingMockToolDispatcher::new("live result");

        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![], &[], None).await;
        assert_eq!(result.unwrap(), "all done");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "tool must be replayed from KV cache, not re-executed"
        );
    }

    /// A fresh (non-recovering) run must execute the tool, persist a checkpoint,
    /// cache the tool result, and mark the promise `Resolved` on completion.
    #[tokio::test]
    async fn run_checkpoints_and_marks_resolved() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::CountingMockToolDispatcher;
        use mock::SequencedMockAnthropicClient;
        use std::sync::atomic::Ordering;

        let store = Arc::new(MockPromiseStore::new());
        // Pre-create the promise so `run()` finds it and writes checkpoints.
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {"k": "v"}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "finished"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let (dispatcher, call_count) = CountingMockToolDispatcher::new("tool output");
        let store_trait = Arc::clone(&store) as Arc<dyn PromiseRepository>;

        let agent = make_durable_agent(anthropic, Arc::new(dispatcher), store_trait, "p1");

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(result.unwrap(), "finished");

        // Tool was actually dispatched once.
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Promise is Resolved.
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);

        // Tool result was cached in KV.
        let ck = super::tool_cache_key("my_tool", &serde_json::json!({"k": "v"}));
        let cached = store.get_tool_result("acme", "p1", &ck).await.unwrap();
        assert_eq!(cached, Some("tool output".to_string()));
    }

    /// A `PermanentFailed` promise must behave identically to a `Resolved` one:
    /// `run()` returns immediately without making any Anthropic call.
    ///
    /// This is the NATS redelivery path for deterministic failures: the process
    /// crashed after marking `PermanentFailed` but before calling `msg.ack()`.
    /// On redelivery we must ack without re-running.
    #[tokio::test]
    async fn permanent_failed_promise_skips_rerun() {
        use crate::promise_store::PromiseStatus;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        let mut p = make_test_promise("p1");
        p.status = PromiseStatus::PermanentFailed;
        store.insert_promise(p);

        // Empty response queue — panics if Anthropic is called at all.
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![]));
        let dispatcher = Arc::new(MockToolDispatcher::new("should not run"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn crate::promise_store::PromiseRepository>,
            "p1",
        );

        let result = agent
            .run(vec![Message::user_text("do stuff")], &[], None)
            .await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "",
            "PermanentFailed promise must return empty string immediately without any LLM call"
        );
    }

    /// A `Failed` promise (transient error) must resume from its checkpoint when
    /// `run()` is called — it must NOT short-circuit like `PermanentFailed`.
    ///
    /// This verifies the core distinction between the two states: `Failed` =
    /// "retry makes sense", `PermanentFailed` = "retry will always fail".
    ///
    /// Scenario: the original run completed turn 1 (tool executed + cached in KV,
    /// checkpoint written), then the turn-2 LLM call returned an HTTP error. The
    /// promise was marked `Failed` with turn-1 messages in the checkpoint. On retry
    /// the agent must load those messages, set `recovering=true`, replay the cached
    /// tool result without re-executing it, and complete normally.
    #[tokio::test]
    async fn failed_promise_resumes_from_checkpoint() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::CountingMockToolDispatcher;
        use mock::SequencedMockAnthropicClient;
        use std::sync::atomic::Ordering;

        let store = Arc::new(MockPromiseStore::new());

        // Checkpoint from turn 1: non-empty messages so `recovering = true`.
        let mut p = make_test_promise("p1");
        p.status = PromiseStatus::Failed;
        p.messages = vec![Message::user_text("start")];
        store.insert_promise(p);

        // Pre-populate the tool result that was cached before the Http error.
        let input = serde_json::json!({"k": "v"});
        let ck = super::tool_cache_key("my_tool", &input);
        store
            .put_tool_result("acme", "p1", &ck, "cached output")
            .await
            .unwrap();

        // Turn 2 retry: LLM requests the same tool again (same input),
        // then ends cleanly.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu-retry", "name": "my_tool", "input": {"k": "v"}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "recovered"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let (dispatcher, call_count) = CountingMockToolDispatcher::new("live result");

        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "recovered",
            "Failed promise must complete normally on retry"
        );

        // Tool must be replayed from KV cache — not re-executed.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "tool must be replayed from cache on Failed promise retry, not re-dispatched"
        );

        // Promise must be Resolved after successful retry.
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// When the agent loop exhausts `max_iterations` without an `end_turn`,
    /// `run()` must mark the promise `PermanentFailed` before returning
    /// `Err(AgentError::MaxIterationsReached)`.
    ///
    /// This is a separate code path from `UnexpectedStopReason`: the loop exits
    /// normally after `max_iterations` iterations, not via a bad `stop_reason`.
    /// Both are deterministic — re-running the same messages with the same limit
    /// will always exhaust again — so `PermanentFailed` is correct for both.
    #[tokio::test]
    async fn max_iterations_reached_marks_promise_permanent_failed() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::ToolContext;
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        // Two tool_use responses with no end_turn — the loop exhausts at
        // max_iterations = 2 without ever completing.
        let tool_use_resp = || {
            serde_json::json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use",
                    "id": "tu_001",
                    "name": "noop",
                    "input": {}
                }]
            })
        };
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp(),
            tool_use_resp(),
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 2,
            tool_dispatcher: dispatcher,
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p1".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert!(
            matches!(result, Err(AgentError::MaxIterationsReached)),
            "expected MaxIterationsReached, got: {result:?}"
        );

        // Promise must be PermanentFailed — same input + same limit = same outcome forever.
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "promise must be PermanentFailed after MaxIterationsReached — retry cannot succeed"
        );
    }

    /// When Anthropic returns an unrecognised `stop_reason` (e.g. `"max_tokens"`),
    /// `run()` must mark the promise `PermanentFailed` before returning the error.
    ///
    /// `PermanentFailed` prevents both startup recovery and NATS redelivery from
    /// retrying the run — deterministic errors produce the same outcome on every
    /// attempt, so retrying wastes Anthropic credits without any benefit.
    #[tokio::test]
    async fn max_tokens_stop_reason_marks_promise_permanent_failed() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        // `max_tokens` is deterministic: the same history will always exhaust
        // the context window, so the run must be PermanentFailed.
        let resp = serde_json::json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "truncated"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![resp]));
        let dispatcher = Arc::new(MockToolDispatcher::new("should not run"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert!(
            matches!(result, Err(AgentError::UnexpectedStopReason(_))),
            "expected UnexpectedStopReason error"
        );

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "max_tokens must be PermanentFailed — same history always hits the limit"
        );
    }

    /// An unknown `stop_reason` (e.g. a future Anthropic API addition) must
    /// mark the promise `Failed` — not `PermanentFailed` — so NATS redelivery
    /// can retry the run. Only `max_tokens` is definitively deterministic.
    #[tokio::test]
    async fn unknown_stop_reason_marks_promise_failed_not_permanent() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let resp = serde_json::json!({
            "stop_reason": "some_future_reason",
            "content": [{"type": "text", "text": ""}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![resp]));
        let dispatcher = Arc::new(MockToolDispatcher::new("should not run"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert!(
            matches!(result, Err(AgentError::UnexpectedStopReason(_))),
            "expected UnexpectedStopReason error"
        );

        // Must be Failed (retryable), not PermanentFailed.
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Failed,
            "unknown stop_reason must be Failed so NATS redelivery can retry"
        );
    }

    // ── Tool execution timeout ────────────────────────────────────────────────

    /// A tool whose dispatch future never resolves must time out after
    /// `TOOL_EXECUTION_TIMEOUT` and feed an error string back to the model.
    ///
    /// Uses Tokio's mock clock (`start_paused = true`) so the test completes
    /// instantly rather than waiting the full 60 real seconds.
    ///
    /// `join!` interleaves the two futures: `agent.run` blocks on the hanging
    /// tool while `advance` moves the mock clock forward, firing the timeout.
    /// The run continues, caches the error result in KV, sends a second request
    /// to Anthropic (which returns `end_turn`), and returns normally.
    #[tokio::test(start_paused = true)]
    async fn hanging_tool_times_out_and_returns_error_string() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use mock::SequencedMockAnthropicClient;
        use std::future::Future;
        use std::pin::Pin;

        struct HangingDispatcher;
        impl crate::tools::ToolDispatcher for HangingDispatcher {
            fn dispatch<'a>(
                &'a self,
                _name: &'a str,
                _input: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
                Box::pin(std::future::pending())
            }
        }

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "slow_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done after timeout"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let agent = make_durable_agent(
            anthropic,
            Arc::new(HangingDispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(TOOL_EXECUTION_TIMEOUT + std::time::Duration::from_secs(1)),
        );

        assert_eq!(
            result.unwrap(),
            "done after timeout",
            "run must complete normally after the tool timeout fires"
        );

        // Timed-out results must NOT be cached — caching them would permanently
        // block the LLM from retrying the tool in future turns.  On crash
        // recovery the tool re-executes (acceptable: it never completed anyway).
        let ck = super::tool_cache_key("slow_tool", &serde_json::json!({}));
        let cached = store.get_tool_result("acme", "p1", &ck).await.unwrap();
        assert!(
            cached.is_none(),
            "timed-out tool result must NOT be cached; got: {cached:?}"
        );
    }

    // ── Checkpoint size guard ─────────────────────────────────────────────────

    /// When the serialized checkpoint would exceed `CHECKPOINT_MAX_BYTES` the
    /// size guard disables further checkpointing for the run.  The run still
    /// completes normally — the tool executes, the model receives the result,
    /// and the final text is returned.  The promise store retains whatever
    /// state it held at the last *successful* checkpoint (none in this test).
    #[tokio::test]
    async fn oversized_checkpoint_disables_checkpointing_but_run_completes() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("tool result"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        // Seed initial messages large enough that `p.messages = messages.clone()`
        // pushes the serialized AgentPromise past CHECKPOINT_MAX_BYTES.
        let large_text = "a".repeat(CHECKPOINT_MAX_BYTES + 1);
        let result = agent
            .run(vec![Message::user_text(large_text)], &[], None)
            .await;

        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete normally even when checkpoint is oversized"
        );

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();

        // Message history was never written — oversized payload skipped.
        assert!(
            p.messages.is_empty(),
            "oversized checkpoint must not be written to the store"
        );
        assert_eq!(
            p.iteration, 0,
            "iteration must not advance when checkpointing is disabled by the size guard"
        );

        // Terminal status IS written — `checkpoint` stays Some so the Resolved
        // write still has a valid KV revision.
        assert_eq!(
            p.status,
            crate::promise_store::PromiseStatus::Resolved,
            "promise must be marked Resolved even when message-history checkpointing is disabled"
        );
    }

    // ── CAS conflict reload ───────────────────────────────────────────────────

    /// When `update_promise` returns a CAS conflict error on the first
    /// checkpoint write, the agent reloads the current revision from KV and
    /// continues the run normally rather than aborting or permanently losing
    /// checkpoint progress.
    ///
    /// The `CasConflictOnceStore` injects exactly one conflict on the first
    /// write, then delegates normally. After the reload, the run proceeds to
    /// the second LLM call and marks the promise `Resolved`.
    #[tokio::test]
    async fn cas_conflict_checkpoint_reload_continues_run() {
        use crate::promise_store::mock::CasConflictOnceStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(CasConflictOnceStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete after CAS conflict reload"
        );

        // Promise must be Resolved in the inner store.
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after run completes despite initial CAS conflict"
        );
    }

    // ── checkpointing retried each turn ───────────────────────────────────────

    /// `checkpointing_disabled` resets to `false` on every tool turn so that a
    /// payload that shrank since the last disabled turn can be written.
    ///
    /// Scenario:
    ///  - turn 1 (3 msgs): payload too large, MIN_TRIM_KEEP=4 prevents any trim → no write.
    ///  - turn 2 (5 msgs): divisor=2 → keep=4, drops the large user message → fits.
    ///    `summarize_dropped_messages` is called for the dropped message (1 LLM call),
    ///    checkpoint written with summary+kept messages.
    ///  - end_turn: `Resolved` written.
    #[tokio::test]
    async fn checkpointing_disabled_persists_across_multiple_turns() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = || {
            serde_json::json!({
                "stop_reason": "tool_use",
                "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
            })
        };
        // Response consumed by summarize_dropped_messages during turn 2 checkpoint.
        let summarize_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "Summary: the agent received a large initial message."}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        // Order: turn-1 LLM, turn-2 LLM, summarize call (inside turn-2 checkpoint), final LLM.
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp(),
            tool_use_resp(),
            summarize_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        // Initial message large enough to exceed CHECKPOINT_MAX_BYTES alone.
        let large_text = "a".repeat(CHECKPOINT_MAX_BYTES + 1);
        let result = agent
            .run(vec![Message::user_text(large_text)], &[], None)
            .await;

        assert_eq!(result.unwrap(), "done");

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        // Turn 1: trimming failed (only 3 messages, can't reduce below MIN_TRIM_KEEP=4).
        // Turn 2: trimming succeeded — large message dropped, checkpoint written.
        assert!(
            !p.messages.is_empty(),
            "checkpoint must be written on turn 2 when trimming drops the large message"
        );
        assert!(
            p.iteration > 0,
            "iteration must advance once checkpoint is written"
        );
        // Terminal write succeeded.
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "Resolved must be written after checkpointing resumes"
        );
    }

    // ── Multiple tools per turn in recovery ───────────────────────────────────

    /// When recovering and the LLM requests multiple tools in a single turn,
    /// each tool is looked up individually in the KV cache. Cached tools are
    /// replayed without dispatching; uncached tools are dispatched normally.
    ///
    /// Scenario: 3 tools in one turn — tool_a and tool_b pre-cached, tool_c not.
    /// The dispatcher must be called exactly once (for tool_c).
    #[tokio::test]
    async fn recovery_replays_multiple_tools_in_same_turn() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::CountingMockToolDispatcher;
        use mock::SequencedMockAnthropicClient;
        use std::sync::atomic::Ordering;

        let store = Arc::new(MockPromiseStore::new());

        // Non-empty checkpoint → recovering = true.
        let mut p = make_test_promise("p1");
        p.messages = vec![Message::user_text("start")];
        store.insert_promise(p);

        // Pre-cache tool_a and tool_b.
        let input_a = serde_json::json!({"k": "a"});
        let input_b = serde_json::json!({"k": "b"});
        let ck_a = super::tool_cache_key("tool_a", &input_a);
        let ck_b = super::tool_cache_key("tool_b", &input_b);
        store
            .put_tool_result("acme", "p1", &ck_a, "result_a")
            .await
            .unwrap();
        store
            .put_tool_result("acme", "p1", &ck_b, "result_b")
            .await
            .unwrap();

        // LLM requests all 3 tools in a single turn, then ends.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [
                {"type": "tool_use", "id": "id_a", "name": "tool_a", "input": {"k": "a"}},
                {"type": "tool_use", "id": "id_b", "name": "tool_b", "input": {"k": "b"}},
                {"type": "tool_use", "id": "id_c", "name": "tool_c", "input": {"k": "c"}}
            ]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "all done"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let (dispatcher, call_count) = CountingMockToolDispatcher::new("live");

        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![], &[], None).await;
        assert_eq!(result.unwrap(), "all done");

        // tool_a and tool_b were replayed from cache — only tool_c was dispatched.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "only the uncached tool must be dispatched; cached tools must be replayed"
        );
    }

    // ── system_prompt persisted in checkpoint ─────────────────────────────────

    /// The system prompt is stored in the first checkpoint so that crash
    /// recovery uses the same context the LLM saw during the original run
    /// rather than a freshly fetched (potentially different) version.
    #[tokio::test]
    async fn system_prompt_stored_in_first_checkpoint() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let system_prompt = "You are a helpful assistant.";
        let result = agent
            .run(vec![Message::user_text("go")], &[], Some(system_prompt))
            .await;
        assert_eq!(result.unwrap(), "done");

        // After the first tool turn the system prompt must be persisted.
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.system_prompt,
            Some(system_prompt.to_string()),
            "system_prompt must be stored in the first checkpoint"
        );
    }

    // ── system_prompt restored from checkpoint on recovery ────────────────────

    /// On crash recovery the agent must use the system prompt stored in the
    /// checkpoint, not the one passed by the caller at recovery time.
    ///
    /// Rationale: the memory file (`.trogon/memory.md`) may have changed
    /// between the crash and the recovery. Using a different system prompt
    /// mid-run would shift the model's behaviour and could cause inconsistent
    /// tool calls. Pinning the prompt to the checkpoint ensures the LLM sees
    /// the same context before and after the crash.
    ///
    /// Scenario:
    ///   1. First run: checkpoint stores `system_prompt = "original prompt"`.
    ///   2. Recovery run: caller passes `system_prompt = "updated prompt"`.
    ///   3. Agent must use `"original prompt"` from the checkpoint, not the
    ///      caller-supplied `"updated prompt"`.
    ///
    /// Verified by inspecting the Anthropic request body serialized by the
    /// mock client, which records the last body it received.
    #[tokio::test]
    async fn recovery_uses_system_prompt_from_checkpoint_not_caller() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;
        use std::sync::Mutex;

        // ── Recording mock client ──────────────────────────────────────────
        // Captures every request body so we can inspect the `system` field.
        struct RecordingAnthropicClient {
            bodies: Arc<Mutex<Vec<serde_json::Value>>>,
            response: serde_json::Value,
        }
        impl AnthropicClient for RecordingAnthropicClient {
            fn complete<'a>(
                &'a self,
                body: serde_json::Value,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<serde_json::Value, AnthropicClientError>>
                        + Send
                        + 'a,
                >,
            > {
                self.bodies.lock().unwrap().push(body);
                let resp = self.response.clone();
                Box::pin(async move { Ok(resp) })
            }
        }

        let store = Arc::new(MockPromiseStore::new());

        // Checkpoint with non-empty messages and the original system prompt.
        let mut p = make_test_promise("p1");
        p.messages = vec![Message::user_text("prior turn")];
        p.system_prompt = Some("original prompt".to_string());
        store.insert_promise(p);

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let bodies: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(vec![]));
        let anthropic = Arc::new(RecordingAnthropicClient {
            bodies: Arc::clone(&bodies),
            response: end_turn_resp,
        });
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        // Caller passes an updated prompt — agent must ignore it and use the
        // one stored in the checkpoint.
        let result = agent
            .run(
                vec![],
                &[],
                Some("updated prompt — must NOT be used on recovery"),
            )
            .await;
        assert_eq!(result.unwrap(), "done");

        // The Anthropic request must contain "original prompt", not the updated one.
        let recorded = bodies.lock().unwrap();
        assert_eq!(recorded.len(), 1, "exactly one Anthropic call expected");
        let system_text = recorded[0]["system"][0]["text"]
            .as_str()
            .expect("system[0].text must be present");
        assert_eq!(
            system_text, "original prompt",
            "recovery must use the system prompt from the checkpoint, not the caller-supplied one"
        );
    }

    // ── tool_cache_key ────────────────────────────────────────────────────────

    /// `tool_cache_key` must produce the same hash regardless of JSON object
    /// key ordering.
    ///
    /// On crash recovery the LLM regenerates the same logical tool call but
    /// may produce different key ordering in the `input` JSON. The cached
    /// result must still match so the tool is not re-executed.
    #[test]
    fn tool_cache_key_is_stable_across_key_orderings() {
        let input_a = serde_json::json!({"z": 1, "a": 2, "m": "hello"});
        let input_b = serde_json::json!({"a": 2, "m": "hello", "z": 1});
        let key_a = tool_cache_key("post_comment", &input_a);
        let key_b = tool_cache_key("post_comment", &input_b);
        assert_eq!(
            key_a, key_b,
            "tool_cache_key must be order-independent for crash-recovery replay"
        );
    }

    /// Different inputs and different tool names must produce different keys.
    #[test]
    fn tool_cache_key_differs_for_different_inputs() {
        let key_1 = tool_cache_key("post_comment", &serde_json::json!({"text": "hello"}));
        let key_2 = tool_cache_key("post_comment", &serde_json::json!({"text": "world"}));
        let key_3 = tool_cache_key("create_issue", &serde_json::json!({"text": "hello"}));
        assert_ne!(key_1, key_2, "different inputs must produce different keys");
        assert_ne!(
            key_1, key_3,
            "different tool names must produce different keys"
        );
    }

    // ── is_retryable_error ────────────────────────────────────────────────────

    /// Spin up a minimal TCP server that responds with the given HTTP status code.
    /// Returns a real `reqwest::Error` for testing the `is_retryable_error` adapter function.
    async fn make_http_error(status: u16) -> reqwest::Error {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let response = format!(
                    "HTTP/1.1 {status} Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        let client = reqwest::Client::new();
        let resp = client.get(format!("http://{addr}/")).send().await.unwrap();
        resp.error_for_status().unwrap_err()
    }

    /// HTTP 5xx errors are retryable — Anthropic-side transient failures.
    #[tokio::test]
    async fn is_retryable_error_true_for_5xx_status() {
        let err = make_http_error(500).await;
        assert!(
            super::is_retryable_error(&err),
            "HTTP 500 must be retryable — server error, not a broken request"
        );
    }

    /// HTTP 4xx errors (except 429, which is handled before reaching this
    /// function) are not retryable — they indicate a deterministic failure in
    /// the request that re-sending will not fix.
    #[tokio::test]
    async fn is_retryable_error_false_for_4xx_status() {
        let err = make_http_error(400).await;
        assert!(
            !super::is_retryable_error(&err),
            "HTTP 400 must not be retryable — broken request, retrying would always fail"
        );
    }

    /// A TCP connection-refused error (ECONNREFUSED) is retryable: it indicates
    /// a transient server-side condition (process restarting, port not yet
    /// listening) that may resolve on the next attempt.
    #[tokio::test]
    async fn is_retryable_error_connect_returns_true() {
        // Bind an ephemeral port, record the address, then immediately drop the
        // listener.  On Linux this reliably produces ECONNREFUSED when we
        // connect to the freed address.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let err = reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("connection to freed port must fail");

        assert!(
            err.is_connect(),
            "ECONNREFUSED must set is_connect()=true; err={err}"
        );
        assert!(
            super::is_retryable_error(&err),
            "connect error must be classified as retryable"
        );
    }

    /// A request that times out while waiting for the server to respond is
    /// retryable — the upstream may simply be slow or temporarily overloaded.
    #[tokio::test]
    async fn is_retryable_error_timeout_returns_true() {
        use std::time::Duration;

        // Server that accepts the connection but never writes a response.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((_stream, _)) = listener.accept().await {
                // Hold the connection open without responding.
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });

        let err = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("request must time out after 50 ms");

        assert!(
            err.is_timeout(),
            "request timeout must set is_timeout()=true; err={err}"
        );
        assert!(
            super::is_retryable_error(&err),
            "timeout error must be classified as retryable"
        );
    }

    // ── retry_after_delay ─────────────────────────────────────────────────────

    /// A valid positive integer in the `Retry-After` header is parsed and
    /// returned directly — no fallback to the 60-second default.
    #[test]
    fn retry_after_delay_returns_parsed_integer_seconds() {
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("retry-after"),
            HeaderValue::from_static("30"),
        );
        assert_eq!(
            super::retry_after_delay(&headers),
            30,
            "valid integer must be returned as-is, not replaced with the 60-s default"
        );
    }

    /// A negative integer string (`"-30"`) fails `parse::<u64>()` (negative
    /// values cannot be represented as unsigned) and the default 60 seconds
    /// is returned.
    #[test]
    fn retry_after_delay_returns_60_for_negative_integer() {
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("retry-after"),
            HeaderValue::from_static("-30"),
        );
        assert_eq!(
            super::retry_after_delay(&headers),
            60,
            "negative integer string must fall back to the 60-s default"
        );
    }

    /// A header value containing non-UTF-8 bytes fails the `.to_str()` call
    /// before reaching `parse::<u64>()`, so the default 60 seconds is returned.
    ///
    /// This exercises the `and_then(|v| v.to_str().ok())` guard in
    /// `retry_after_delay`.
    #[test]
    fn retry_after_delay_returns_60_for_non_utf8_header() {
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
        let mut headers = HeaderMap::new();
        // 0xFF is valid in an HTTP header value (obs-text) but is not valid
        // UTF-8, so `.to_str()` returns `Err` and we fall back to the default.
        headers.insert(
            HeaderName::from_static("retry-after"),
            HeaderValue::from_bytes(b"\xff").unwrap(),
        );
        assert_eq!(
            super::retry_after_delay(&headers),
            60,
            "non-UTF-8 header value must fall back to the 60-s default"
        );
    }

    // ── HTTP error classification ─────────────────────────────────────────────

    struct FixedErrorClient(AnthropicClientError);
    impl AnthropicClient for FixedErrorClient {
        fn complete<'a>(
            &'a self,
            _body: serde_json::Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<serde_json::Value, AnthropicClientError>>
                    + Send
                    + 'a,
            >,
        > {
            let e = self.0.clone();
            Box::pin(async move { Err(e) })
        }
    }

    /// A non-retryable 4xx HTTP error (status 400) must write `PermanentFailed`
    /// to the promise store so that neither startup recovery nor NATS
    /// redelivery wastes Anthropic credits on a deterministically broken run.
    #[tokio::test]
    async fn http_4xx_error_marks_promise_permanent_failed() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let agent = make_durable_agent(
            Arc::new(FixedErrorClient(AnthropicClientError::Permanent("400 Bad Request".into()))),
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert!(matches!(result, Err(AgentError::Http(_))));

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "4xx error must mark promise PermanentFailed — retrying would always fail"
        );
    }

    /// A 5xx HTTP error must write `Failed` so NATS redelivery can retry the
    /// run from its checkpoint once the upstream service recovers.
    #[tokio::test]
    async fn http_5xx_error_marks_promise_failed() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let agent = make_durable_agent(
            Arc::new(FixedErrorClient(AnthropicClientError::Transient("500 Internal Server Error".into()))),
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert!(matches!(result, Err(AgentError::Http(_))));

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Failed,
            "5xx error must mark promise Failed so NATS redelivery can retry"
        );
    }

    // ── CAS reload edge cases ─────────────────────────────────────────────────

    /// When the CAS conflict reload finds the promise is already `Resolved` by
    /// another worker, the run must stop immediately and return `Ok("")` to
    /// avoid duplicate tool side-effects (extra PR comments, Slack messages).
    #[tokio::test]
    async fn cas_reload_terminal_promise_by_other_worker_stops_run() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::TerminalOnCasReloadStore;
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(TerminalOnCasReloadStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        // First LLM call returns tool_use to trigger the checkpoint + CAS
        // conflict path. The second response must never be consumed.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let unreachable_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "must not reach here"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            unreachable_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "",
            "run must stop when CAS reload finds another worker already resolved the promise"
        );
    }

    /// When the CAS conflict reload finds the promise has disappeared from KV,
    /// `checkpoint` is set to `None` — checkpointing is disabled for the
    /// remainder of the run and the run still completes normally.
    #[tokio::test]
    async fn cas_reload_promise_disappeared_disables_checkpointing_and_run_completes() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::DisappearedOnCasReloadStore;
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(DisappearedOnCasReloadStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "finished despite disappeared promise"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "finished despite disappeared promise",
            "run must complete normally even when the promise disappears during CAS reload"
        );
    }

    /// When a CAS conflict occurs and the subsequent `get_promise` reload times
    /// out, `unwrap_or_else(|_| Ok(None))` converts the timeout to `Ok(None)`.
    /// The `Ok(None)` arm then sets `checkpoint = None`, disabling further
    /// checkpointing while the run still completes normally.
    ///
    /// Distinct from `cas_reload_promise_disappeared_disables_checkpointing_and_run_completes`
    /// which uses `DisappearedOnCasReloadStore` (returns `Ok(None)` directly).
    /// This test exercises the `unwrap_or_else` closure path that logs a
    /// different warning before falling through to the same `Ok(None)` arm.
    #[tokio::test(start_paused = true)]
    async fn cas_reload_get_promise_timeout_disables_checkpointing_and_run_completes() {
        use crate::agent_loop::NATS_KV_TIMEOUT;
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::HangingCasReloadStore;
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;
        use std::time::Duration;

        let store = Arc::new(HangingCasReloadStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "finished despite reload timeout"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );

        assert_eq!(
            result.unwrap(),
            "finished despite reload timeout",
            "run must complete normally even when the CAS reload get_promise times out"
        );
    }

    // ── Deserialization error ─────────────────────────────────────────────────

    /// When Anthropic returns valid JSON that doesn't match `AnthropicResponse`
    /// (e.g. missing `stop_reason` and `content`), the agent must mark the
    /// promise `PermanentFailed` — retrying the same malformed response will
    /// always fail, so redelivery would only waste credits.
    #[tokio::test]
    async fn deserialization_error_marks_promise_permanent_failed() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        // Valid JSON but not a valid AnthropicResponse — missing required fields.
        let malformed = serde_json::json!({"unexpected_field": "not a response"});
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![malformed]));
        let dispatcher = Arc::new(MockToolDispatcher::new("unused"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        match &result {
            Err(AgentError::UnexpectedStopReason(s)) => {
                assert!(
                    s.contains("Deserialization"),
                    "error must mention Deserialization; got: {s}"
                );
            }
            other => panic!("expected UnexpectedStopReason, got: {other:?}"),
        };

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "deserialization error must mark promise PermanentFailed"
        );
    }

    // ── _idempotency_key injection ────────────────────────────────────────────

    /// When `promise_id` is set, built-in tools must receive an `_idempotency_key`
    /// field injected into their input.  The key must be stable — derived from
    /// `{promise_id}.{sha256(tool_name:input)}` — so that Slack/Linear dedup
    /// checks fire correctly on retry even after a process restart.
    #[tokio::test]
    async fn idempotency_key_injected_into_builtin_tool_input() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Mutex;

        struct CapturingDispatcher {
            last_input: Arc<Mutex<Option<serde_json::Value>>>,
        }
        impl crate::tools::ToolDispatcher for CapturingDispatcher {
            fn dispatch<'a>(
                &'a self,
                _name: &'a str,
                input: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
                *self.last_input.lock().unwrap() = Some(input.clone());
                Box::pin(async { "ok".to_string() })
            }
        }

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "post_comment", "input": {"body": "hello"}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let captured = Arc::new(Mutex::new(None::<serde_json::Value>));
        let dispatcher = Arc::new(CapturingDispatcher {
            last_input: Arc::clone(&captured),
        });

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        let input = captured
            .lock()
            .unwrap()
            .take()
            .expect("dispatcher was never called");
        let ikey = input["_idempotency_key"]
            .as_str()
            .expect("_idempotency_key must be present in tool input");

        // Key format: {promise_id}.{sha256(tool_name:canonical_json(input))}
        assert!(
            ikey.starts_with("p1."),
            "_idempotency_key must start with the promise_id; got: {ikey}"
        );

        // The hash part must be a 64-char lowercase hex SHA-256.
        let hash_part = ikey.trim_start_matches("p1.");
        assert_eq!(
            hash_part.len(),
            64,
            "hash must be 64 hex chars; got: {hash_part}"
        );
        assert!(
            hash_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hash must be lowercase hex; got: {hash_part}"
        );

        // Stability: the key is content-based, so calling with the same input
        // again must produce the identical key.
        let expected_key = format!(
            "p1.{}",
            tool_cache_key("post_comment", &serde_json::json!({"body": "hello"}))
        );
        assert_eq!(
            ikey, expected_key,
            "_idempotency_key must be deterministic across retries"
        );
    }

    // ── update_promise KV timeout → checkpoint = None ─────────────────────────

    /// When the `update_promise` KV write exceeds `NATS_KV_TIMEOUT`, `checkpoint`
    /// is set to `None` and the run continues to completion without further
    /// checkpoint attempts.
    ///
    /// This prevents the run from blocking indefinitely while the NATS heartbeat
    /// keeps the message alive. The worst case is that a subsequent crash causes
    /// the run to restart from scratch rather than from a checkpoint.
    ///
    /// Uses Tokio's mock clock so the test completes instantly.
    #[tokio::test(start_paused = true)]
    async fn update_promise_kv_timeout_disables_checkpointing_and_run_completes() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::HangingUpdateStore;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(HangingUpdateStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done despite kv timeout"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + std::time::Duration::from_millis(1)),
        );

        assert_eq!(
            result.unwrap(),
            "done despite kv timeout",
            "run must complete normally even when the KV checkpoint write times out"
        );

        // Promise was never updated — status is still Running.
        // (checkpoint = None means the terminal Resolved write was also skipped.)
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            crate::promise_store::PromiseStatus::Running,
            "promise must remain Running when checkpoint write timed out — terminal write is also skipped"
        );
    }

    // ── MCP tool idempotency key ──────────────────────────────────────────────

    /// MCP tools must receive the original `input` without `_idempotency_key`
    /// injected.  MCP servers may declare `additionalProperties: false` in their
    /// tool schema and would reject an unknown field with a hard error.
    ///
    /// Built-in tools receive `call_input` (with the key); MCP tools receive
    /// `input` (without it).  This test guards against accidentally swapping
    /// the two.
    #[tokio::test]
    async fn mcp_tool_does_not_receive_idempotency_key() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::{ToolContext, ToolDispatcher, tool_def};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Mutex;

        struct CapturingMcpClient {
            last_input: Arc<Mutex<Option<serde_json::Value>>>,
        }
        impl trogon_mcp::McpCallTool for CapturingMcpClient {
            fn call_tool<'a>(
                &'a self,
                _name: &'a str,
                arguments: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
                *self.last_input.lock().unwrap() = Some(arguments.clone());
                Box::pin(async { Ok("mcp result".to_string()) })
            }
        }

        // Stub built-in dispatcher — must not be called for an MCP tool.
        struct PanickingDispatcher;
        impl ToolDispatcher for PanickingDispatcher {
            fn dispatch<'a>(
                &'a self,
                name: &'a str,
                _input: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
                panic!("built-in dispatcher must not be called for MCP tool '{name}'");
            }
        }

        let captured = Arc::new(Mutex::new(None::<serde_json::Value>));
        let mcp_client: Arc<dyn trogon_mcp::McpCallTool> = Arc::new(CapturingMcpClient {
            last_input: Arc::clone(&captured),
        });

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        // LLM calls the prefixed MCP tool name.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "mcp__srv__search", "input": {"query": "rust"}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(PanickingDispatcher),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![tool_def(
                "mcp__srv__search",
                "search",
                serde_json::json!({"type": "object"}),
            )],
            mcp_dispatch: vec![(
                "mcp__srv__search".to_string(),
                "search".to_string(),
                mcp_client,
            )],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p1".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        };

        agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        let input = captured
            .lock()
            .unwrap()
            .take()
            .expect("MCP client was never called");
        assert!(
            input.get("_idempotency_key").is_none(),
            "_idempotency_key must NOT be injected into MCP tool input; got: {input}"
        );
        assert_eq!(
            input["query"].as_str().unwrap(),
            "rust",
            "original input fields must be preserved"
        );
    }

    // ── get_tool_result KV timeout ────────────────────────────────────────────

    /// When `get_tool_result` times out during recovery, the run skips tool
    /// execution and injects an error ToolResult instead.  This prevents
    /// duplicate side effects when the previous execution may have already
    /// completed — the LLM receives a message telling it to stop the run.
    #[tokio::test(start_paused = true)]
    async fn get_tool_result_kv_timeout_skips_execution_and_injects_error() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::HangingGetToolResultStore;
        use crate::tools::mock::CountingMockToolDispatcher;
        use std::sync::atomic::Ordering;

        let store = Arc::new(HangingGetToolResultStore::new());

        // Non-empty checkpoint messages → `recovering = true`.
        let mut p = make_test_promise("p1");
        p.messages = vec![Message::user_text("prior turn")];
        store.inner.insert_promise(p);

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "stopping due to cache error"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let (dispatcher, call_count) = CountingMockToolDispatcher::new("live result");

        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + std::time::Duration::from_millis(1)),
        );

        assert_eq!(result.unwrap(), "stopping due to cache error");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "tool must NOT be re-executed when get_tool_result KV read times out — \
             previous execution may have already produced side effects"
        );
    }

    // ── put_tool_result KV timeout ────────────────────────────────────────────

    /// When `put_tool_result` times out, the run continues normally — but the
    /// tool result is not persisted.  A subsequent crash would cause the tool
    /// to be re-executed on recovery rather than replayed from cache.
    #[tokio::test(start_paused = true)]
    async fn put_tool_result_kv_timeout_run_completes_but_result_not_cached() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::HangingPutToolResultStore;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(HangingPutToolResultStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done despite put timeout"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("tool output"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            async {
                // Fire the first put_tool_result timeout.
                tokio::time::advance(NATS_KV_TIMEOUT + std::time::Duration::from_millis(1)).await;
                // Fire the retry timeout (HangingPutToolResultStore hangs both calls).
                tokio::time::advance(NATS_KV_TIMEOUT + std::time::Duration::from_millis(1)).await;
            },
        );

        assert_eq!(result.unwrap(), "done despite put timeout");

        // Tool result was NOT persisted — both put_tool_result attempts timed out.
        let ck = tool_cache_key("my_tool", &serde_json::json!({}));
        let cached = store
            .inner
            .get_tool_result("acme", "p1", &ck)
            .await
            .unwrap();
        assert!(
            cached.is_none(),
            "tool result must not be cached when put_tool_result times out"
        );
    }

    // ── Non-durable mode ─────────────────────────────────────────────────────

    /// When `promise_store` is `None` the durable promise block is skipped
    /// entirely and the run completes normally.  This guards the backward-
    /// compatible non-durable code path that existed before checkpointing was
    /// added.
    #[tokio::test]
    async fn non_durable_run_completes_without_promise_store() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "non-durable result"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();
        assert_eq!(result, "non-durable result");
    }

    // ── recovering flag reset between turns ───────────────────────────────────

    /// After the first tool turn, `recovering` resets to `false`.  A second
    /// tool turn must dispatch the tool live — the cache is not consulted for
    /// turns after the recovery catch-up point.
    ///
    /// Scenario:
    ///   1. Checkpoint with non-empty messages → `recovering = true`.
    ///   2. Turn 1: `cached_tool` result is in KV → replayed without dispatch.
    ///   3. `recovering` resets to `false`.
    ///   4. Turn 2: `live_tool` result is NOT in KV, but cache is never checked
    ///      (recovering=false) → dispatched live.
    ///   5. Turn 3: end_turn.
    ///
    /// The dispatcher must be called exactly once (only turn 2).
    #[tokio::test]
    async fn recovering_flag_resets_after_first_tool_turn() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::CountingMockToolDispatcher;
        use std::sync::atomic::Ordering;

        let store = Arc::new(MockPromiseStore::new());

        // Non-empty checkpoint → recovering = true.
        let mut p = make_test_promise("p1");
        p.messages = vec![Message::user_text("prior")];
        store.insert_promise(p);

        // Pre-cache only the turn-1 tool.
        let ck = tool_cache_key("cached_tool", &serde_json::json!({}));
        store
            .put_tool_result("acme", "p1", &ck, "cached output")
            .await
            .unwrap();

        let resp_turn1 = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "id1", "name": "cached_tool", "input": {}}]
        });
        let resp_turn2 = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "id2", "name": "live_tool", "input": {}}]
        });
        let resp_end = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            resp_turn1, resp_turn2, resp_end,
        ]));
        let (dispatcher, call_count) = CountingMockToolDispatcher::new("live result");

        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![], &[], None).await.unwrap();
        assert_eq!(result, "done");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "only turn 2 must be dispatched; turn 1 was recovered from cache"
        );
    }

    // ── Failed promise with empty messages ────────────────────────────────────

    /// A `Failed` promise with no checkpoint messages must start the run from
    /// scratch using the caller-supplied `initial_messages`.
    ///
    /// This covers a crash that happened between the initial promise creation
    /// and the first checkpoint write — before any LLM turn completed.
    /// `recovering` must stay `false` so tools are dispatched live (no cache
    /// lookup), and the run uses the caller's messages, not an empty history.
    #[tokio::test]
    async fn failed_promise_with_empty_messages_starts_fresh() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::CountingMockToolDispatcher;
        use std::sync::atomic::Ordering;

        let store = Arc::new(MockPromiseStore::new());

        // Failed promise with no checkpoint messages.
        let mut p = make_test_promise("p1");
        p.status = PromiseStatus::Failed;
        p.messages = vec![]; // no checkpoint
        store.insert_promise(p);

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "fresh start result"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let (dispatcher, call_count) = CountingMockToolDispatcher::new("tool output");

        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        // Pass explicit initial messages to verify the run starts from them,
        // not from an empty history.
        let result = agent
            .run(
                vec![Message::user_text("caller initial message")],
                &[],
                None,
            )
            .await
            .unwrap();
        assert_eq!(result, "fresh start result");

        // Tool was dispatched live — no recovery cache lookup.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "tool must be dispatched live when Failed promise has no checkpoint messages"
        );

        // Promise is now Resolved.
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    // ── Initial get_promise failures ─────────────────────────────────────────

    /// When the initial `get_promise` KV read times out on both attempts,
    /// `run()` must return `Err(AgentError::CheckpointLoadTimeout)` rather
    /// than proceeding without checkpoint durability.
    ///
    /// Proceeding silently would risk re-executing tool calls that were
    /// already cached in a previous delivery. Returning an error leaves the
    /// promise in `Running` state so startup recovery can resume it once
    /// KV is healthy again.
    #[tokio::test(start_paused = true)]
    async fn initial_get_promise_timeout_returns_error() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::HangingGetPromiseStore;
        use crate::tools::mock::MockToolDispatcher;
        use std::time::Duration;

        let store = Arc::new(HangingGetPromiseStore::new());
        // No Anthropic response needed — run aborts before any LLM call.
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![]));
        let dispatcher = Arc::new(MockToolDispatcher::new("unused"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        // Advance past both timeout attempts (each NATS_KV_TIMEOUT) and the
        // 200 ms sleep between them.
        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("initial")], &[], None),
            tokio::time::advance(2 * NATS_KV_TIMEOUT + Duration::from_millis(300)),
        );

        assert!(
            matches!(result, Err(AgentError::CheckpointLoadTimeout)),
            "run must return CheckpointLoadTimeout when initial KV load times out after retries"
        );
    }

    /// When the initial `get_promise` KV read returns an error, the run must
    /// start fresh from the caller's `initial_messages` rather than returning
    /// the error to the caller.
    #[tokio::test]
    async fn initial_get_promise_error_starts_fresh() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::ErrorGetPromiseStore;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(ErrorGetPromiseStore::new());

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "fresh run after error"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));
        let dispatcher = Arc::new(MockToolDispatcher::new("unused"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent
            .run(vec![Message::user_text("initial")], &[], None)
            .await
            .unwrap();
        assert_eq!(
            result, "fresh run after error",
            "run must start fresh when initial KV read returns an error"
        );
    }

    // ── effective_prompt fallback ─────────────────────────────────────────────

    /// When recovering from a checkpoint that has no stored `system_prompt`
    /// (e.g. a promise written before the field was introduced), the agent
    /// must fall back to the caller-supplied prompt rather than using `None`.
    ///
    /// This ensures the LLM is not silently run without a system prompt after
    /// a schema migration.
    #[tokio::test]
    async fn effective_prompt_falls_back_to_caller_when_checkpoint_has_no_system_prompt() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;
        use std::sync::Mutex;

        struct RecordingAnthropicClient {
            bodies: Arc<Mutex<Vec<serde_json::Value>>>,
            response: serde_json::Value,
        }
        impl AnthropicClient for RecordingAnthropicClient {
            fn complete<'a>(
                &'a self,
                body: serde_json::Value,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<serde_json::Value, AnthropicClientError>>
                        + Send
                        + 'a,
                >,
            > {
                self.bodies.lock().unwrap().push(body);
                let resp = self.response.clone();
                Box::pin(async move { Ok(resp) })
            }
        }

        let store = Arc::new(MockPromiseStore::new());

        // Checkpoint with non-empty messages but NO system_prompt.
        let mut p = make_test_promise("p1");
        p.messages = vec![Message::user_text("prior turn")];
        p.system_prompt = None; // old promise — field not persisted
        store.insert_promise(p);

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let bodies: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(vec![]));
        let anthropic = Arc::new(RecordingAnthropicClient {
            bodies: Arc::clone(&bodies),
            response: end_turn_resp,
        });

        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("ok")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        agent
            .run(vec![], &[], Some("caller fallback prompt"))
            .await
            .unwrap();

        let recorded = bodies.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        let system_text = recorded[0]["system"][0]["text"]
            .as_str()
            .expect("system[0].text must be present");
        assert_eq!(
            system_text, "caller fallback prompt",
            "must fall back to caller-supplied prompt when checkpoint has no system_prompt"
        );
    }

    // ── MCP tool error ────────────────────────────────────────────────────────

    /// When an MCP server returns an error from `call_tool`, the error string
    /// must be forwarded to the model as the tool result so the agent can
    /// decide how to handle it.  The run must not abort — the error is treated
    /// like any other tool output.
    #[tokio::test]
    async fn mcp_tool_error_is_forwarded_to_model_as_tool_result() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::{ToolContext, ToolDispatcher, tool_def};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Mutex;

        struct ErrorMcpClient;
        impl trogon_mcp::McpCallTool for ErrorMcpClient {
            fn call_tool<'a>(
                &'a self,
                _name: &'a str,
                _arguments: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
                Box::pin(async { Err("tool not found on server".to_string()) })
            }
        }

        // Capture the second Anthropic request to inspect the tool-result message.
        struct RecordingClient {
            calls: Arc<Mutex<Vec<serde_json::Value>>>,
            responses: Mutex<std::collections::VecDeque<serde_json::Value>>,
        }
        impl AnthropicClient for RecordingClient {
            fn complete<'a>(
                &'a self,
                body: serde_json::Value,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<serde_json::Value, AnthropicClientError>>
                        + Send
                        + 'a,
                >,
            > {
                self.calls.lock().unwrap().push(body);
                let resp = self
                    .responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("no more responses");
                Box::pin(async move { Ok(resp) })
            }
        }

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "mcp__srv__search", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "handled error"}]
        });

        let calls: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(vec![]));
        let anthropic = Arc::new(RecordingClient {
            calls: Arc::clone(&calls),
            responses: Mutex::new([tool_use_resp, end_turn_resp].into()),
        });

        struct NeverDispatcher;
        impl ToolDispatcher for NeverDispatcher {
            fn dispatch<'a>(
                &'a self,
                name: &'a str,
                _input: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
                panic!("built-in dispatcher must not be called; tool '{name}' is MCP");
            }
        }

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(NeverDispatcher),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![tool_def(
                "mcp__srv__search",
                "search",
                serde_json::json!({"type": "object"}),
            )],
            mcp_dispatch: vec![(
                "mcp__srv__search".to_string(),
                "search".to_string(),
                Arc::new(ErrorMcpClient) as Arc<dyn trogon_mcp::McpCallTool>,
            )],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p1".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();
        assert_eq!(result, "handled error");

        // The second Anthropic call must carry the MCP error as the tool result.
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 2, "expected exactly 2 Anthropic calls");
        let messages = recorded[1]["messages"].as_array().unwrap();
        let last_msg = messages.last().unwrap();
        // Tool results are in a user message with ToolResult content blocks.
        let tool_result_content = last_msg["content"][0]["content"]
            .as_str()
            .expect("tool result content must be a string");
        assert!(
            tool_result_content.starts_with("Tool error:"),
            "MCP error must be prefixed 'Tool error:'; got: {tool_result_content}"
        );
        assert!(
            tool_result_content.contains("tool not found on server"),
            "MCP error message must be included; got: {tool_result_content}"
        );
    }

    // ── get_tool_result Err during recovery ──────────────────────────────────

    /// When `get_tool_result` returns an error (not timeout) during recovery,
    /// the run skips tool execution and injects an error ToolResult.  Re-executing
    /// the tool risks duplicate side effects if the prior run already completed it.
    #[tokio::test]
    async fn get_tool_result_error_skips_execution_and_injects_error() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::ErrorGetToolResultStore;
        use crate::tools::mock::CountingMockToolDispatcher;
        use std::sync::atomic::Ordering;

        let store = Arc::new(ErrorGetToolResultStore::new());

        // Non-empty checkpoint → recovering = true.
        let mut p = make_test_promise("p1");
        p.messages = vec![Message::user_text("prior turn")];
        store.inner.insert_promise(p);

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "stopping due to cache error"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let (dispatcher, call_count) = CountingMockToolDispatcher::new("live result");

        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![], &[], None).await.unwrap();
        assert_eq!(result, "stopping due to cache error");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "tool must NOT be re-executed when get_tool_result returns an error — \
             previous execution may have already produced side effects"
        );
    }

    // ── MCP tool execution timeout ────────────────────────────────────────────

    /// When an MCP `call_tool` hangs past `TOOL_EXECUTION_TIMEOUT`, the run
    /// must continue and the model must receive a timed-out error string.
    #[tokio::test(start_paused = true)]
    async fn mcp_tool_execution_timeout_returns_error_string() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::{ToolContext, ToolDispatcher, tool_def};
        use std::future::Future;
        use std::pin::Pin;

        struct HangingMcpClient;
        impl trogon_mcp::McpCallTool for HangingMcpClient {
            fn call_tool<'a>(
                &'a self,
                _name: &'a str,
                _arguments: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
                Box::pin(std::future::pending())
            }
        }

        struct NeverDispatcher;
        impl ToolDispatcher for NeverDispatcher {
            fn dispatch<'a>(
                &'a self,
                name: &'a str,
                _input: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
                panic!("built-in dispatcher must not be called for MCP tool '{name}'");
            }
        }

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "mcp__srv__search", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done after mcp timeout"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(NeverDispatcher),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![tool_def(
                "mcp__srv__search",
                "search",
                serde_json::json!({"type": "object"}),
            )],
            mcp_dispatch: vec![(
                "mcp__srv__search".to_string(),
                "search".to_string(),
                Arc::new(HangingMcpClient) as Arc<dyn trogon_mcp::McpCallTool>,
            )],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p1".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        };

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(TOOL_EXECUTION_TIMEOUT + std::time::Duration::from_secs(1)),
        );

        assert_eq!(result.unwrap(), "done after mcp timeout");

        // Timed-out results must NOT be cached — caching them would permanently
        // block the LLM from retrying the MCP tool in future turns.
        let ck = tool_cache_key("mcp__srv__search", &serde_json::json!({}));
        let cached = store.get_tool_result("acme", "p1", &ck).await.unwrap();
        assert!(
            cached.is_none(),
            "timed-out MCP result must NOT be cached; got: {cached:?}"
        );
    }

    // ── put_tool_result Err ───────────────────────────────────────────────────

    /// When `put_tool_result` returns an error (not timeout), the run continues
    /// normally — the error is logged but does not abort. The result is not
    /// cached, so a crash would cause re-execution on recovery.
    #[tokio::test]
    async fn put_tool_result_error_run_completes_but_result_not_cached() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::ErrorPutToolResultStore;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(ErrorPutToolResultStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done despite put error"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("tool output")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();
        assert_eq!(result, "done despite put error");

        // Result was not persisted — put failed.
        let ck = tool_cache_key("my_tool", &serde_json::json!({}));
        let cached = store
            .inner
            .get_tool_result("acme", "p1", &ck)
            .await
            .unwrap();
        assert!(
            cached.is_none(),
            "tool result must not be cached when put_tool_result returns an error"
        );
    }

    // ── CAS reload get_promise Err → keep revision, terminal write still works ──

    /// When `get_promise` returns an error during the CAS conflict reload,
    /// the checkpoint revision is preserved (not set to None). The terminal
    /// Resolved write therefore succeeds using the still-valid old revision.
    ///
    /// This is the fix for #11: an unreachable NATS during reload no longer
    /// permanently blocks terminal-status writes for the rest of the run.
    #[tokio::test]
    async fn cas_reload_get_promise_error_disables_checkpointing_and_run_completes() {
        use crate::promise_store::mock::ErrorReloadStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(ErrorReloadStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done despite reload error"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("ok")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();
        assert_eq!(
            result, "done despite reload error",
            "run must complete normally when CAS reload get_promise returns an error"
        );

        // With Fix #11: reload error keeps the old revision. The inner store never
        // wrote a new revision (the CAS conflict was injected), so the old rev is
        // still valid. The terminal Resolved write therefore succeeds.
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved — terminal write succeeds with preserved revision"
        );
    }

    // ── retry_after_delay ─────────────────────────────────────────────────────

    /// Valid integer-seconds value → parsed directly.
    #[test]
    fn retry_after_delay_parses_integer_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "30".parse().unwrap());
        assert_eq!(retry_after_delay(&headers), 30);
    }

    /// Header absent → falls back to 60 s default.
    #[test]
    fn retry_after_delay_defaults_to_60_when_header_absent() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after_delay(&headers), 60);
    }

    /// HTTP-date value (not an integer) → unparseable, falls back to 60 s.
    #[test]
    fn retry_after_delay_defaults_to_60_for_http_date() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_delay(&headers), 60);
    }

    /// Zero is a valid parsed value — callers must handle a zero delay.
    #[test]
    fn retry_after_delay_allows_zero() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "0".parse().unwrap());
        assert_eq!(retry_after_delay(&headers), 0);
    }

    // ── write_promise_terminal KV timeout ─────────────────────────────────────

    /// When the `update_promise` call inside `write_promise_terminal` hangs past
    /// `NATS_KV_TIMEOUT`, the function times out, logs a warning, and returns
    /// without updating the promise.  The run still returns the error to the
    /// caller — the timeout does NOT swallow the original error.
    ///
    /// Uses Tokio's mock clock so the test completes instantly.
    #[tokio::test(start_paused = true)]
    async fn write_promise_terminal_kv_timeout_leaves_promise_running() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::HangingUpdateStore;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(HangingUpdateStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        // "max_tokens" is an unrecognised stop_reason that goes directly to the
        // `write_promise_terminal` path without any prior checkpoint write.
        let bad_resp = serde_json::json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "truncated"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![bad_resp]));
        let dispatcher = Arc::new(MockToolDispatcher::new("unused"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + std::time::Duration::from_millis(1)),
        );

        assert!(
            matches!(result, Err(AgentError::UnexpectedStopReason(_))),
            "run must still return UnexpectedStopReason when terminal write times out; got: {result:?}"
        );

        // update_promise never returned — promise status is still Running.
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            crate::promise_store::PromiseStatus::Running,
            "promise must remain Running when write_promise_terminal KV write times out"
        );
    }

    // ── sort_json_keys — nested and array inputs ──────────────────────────────

    /// `sort_json_keys` must recursively sort nested object keys so the cache
    /// key is identical regardless of deep key ordering.
    #[test]
    fn tool_cache_key_stable_for_deeply_nested_objects() {
        let input_a = serde_json::json!({"outer": {"z": 3, "a": 1, "m": {"y": 9, "x": 7}}});
        let input_b = serde_json::json!({"outer": {"a": 1, "m": {"x": 7, "y": 9}, "z": 3}});
        assert_eq!(
            tool_cache_key("t", &input_a),
            tool_cache_key("t", &input_b),
            "nested object key ordering must not affect the cache key"
        );
    }

    /// Arrays of objects: element order is preserved but each object's keys are
    /// sorted, so two inputs differing only in intra-object key order produce
    /// the same cache key.
    #[test]
    fn tool_cache_key_stable_for_arrays_of_objects() {
        let input_a = serde_json::json!([{"z": 1, "a": 2}, {"b": 3, "c": 4}]);
        let input_b = serde_json::json!([{"a": 2, "z": 1}, {"c": 4, "b": 3}]);
        assert_eq!(
            tool_cache_key("t", &input_a),
            tool_cache_key("t", &input_b),
            "intra-object key order within array elements must not affect the cache key"
        );
    }

    // ── run_chat ──────────────────────────────────────────────────────────────

    /// `run_chat` on `end_turn` returns the text content and the full updated
    /// message history, including the appended assistant turn.
    #[tokio::test]
    async fn run_chat_end_turn_returns_text_and_updated_messages() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hello back"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let (text, messages) = agent
            .run_chat(vec![Message::user_text("hello")], &[], None)
            .await
            .unwrap();

        assert_eq!(text, "hello back");
        // [user("hello"), assistant("hello back")]
        assert_eq!(
            messages.len(),
            2,
            "expected 2 messages: initial user + assistant end_turn"
        );
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
        assert!(
            matches!(&messages[1].content[0], ContentBlock::Text { text } if text == "hello back"),
            "final assistant message must contain the response text"
        );
    }

    /// `run_chat` with a tool_use turn extends the history with all exchanges:
    /// initial user, assistant tool_use, user tool_results, assistant end_turn.
    #[tokio::test]
    async fn run_chat_tool_use_extends_messages() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "all done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("tool result")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let (text, messages) = agent
            .run_chat(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        assert_eq!(text, "all done");
        // [user("go"), assistant(tool_use), user(tool_result), assistant(end_turn)]
        assert_eq!(
            messages.len(),
            4,
            "expected 4 messages: initial + tool_use + tool_result + end_turn"
        );
        assert_eq!(messages[0].role, "user"); // initial
        assert_eq!(messages[1].role, "assistant"); // tool_use
        assert_eq!(messages[2].role, "user"); // tool_result
        assert_eq!(messages[3].role, "assistant"); // end_turn
        assert!(
            matches!(&messages[3].content[0], ContentBlock::Text { text } if text == "all done"),
            "final assistant message must contain 'all done'"
        );
    }

    /// `run_chat` returns `Err(MaxIterationsReached)` when the loop exhausts
    /// its iteration budget without ever reaching `end_turn`.
    #[tokio::test]
    async fn run_chat_max_iterations_returns_error() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        // One tool_use response and max_iterations = 1 — loop exhausts immediately.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![tool_use_resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("ok")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run_chat(vec![Message::user_text("go")], &[], None)
            .await;

        assert!(
            matches!(result, Err(AgentError::MaxIterationsReached)),
            "expected MaxIterationsReached when all iterations are tool_use; got: {result:?}"
        );
    }

    // ── run_chat error paths ──────────────────────────────────────────────────

    /// `run_chat` must propagate HTTP errors as `Err(AgentError::Http)`.
    ///
    /// Unlike `run()`, `run_chat()` has no promise store interaction — the
    /// error just bubbles straight up via `map_err(AgentError::Http)?`.
    #[tokio::test]
    async fn run_chat_http_error_returns_error() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};
        use std::future::Future;
        use std::pin::Pin;

        struct ErrorClient;
        impl AnthropicClient for ErrorClient {
            fn complete<'a>(
                &'a self,
                _body: serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
            {
                Box::pin(async move { Err(AnthropicClientError::Transient("simulated error".into())) })
            }
        }

        let agent = AgentLoop {
            anthropic_client: Arc::new(ErrorClient),
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run_chat(vec![Message::user_text("hello")], &[], None)
            .await;

        assert!(
            matches!(result, Err(AgentError::Http(_))),
            "run_chat must return Err(AgentError::Http) on Anthropic HTTP error; got: {result:?}"
        );
    }

    /// `run_chat` must return `Err(UnexpectedStopReason("Deserialization error: ..."))`
    /// when the Anthropic response JSON cannot be deserialized into `AnthropicResponse`.
    #[tokio::test]
    async fn run_chat_deserialization_error_returns_error() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        let malformed = serde_json::json!({"not_a_response": true});
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![malformed]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run_chat(vec![Message::user_text("hello")], &[], None)
            .await;

        match &result {
            Err(AgentError::UnexpectedStopReason(s)) => {
                assert!(
                    s.contains("Deserialization"),
                    "error must mention Deserialization; got: {s}"
                );
            }
            other => panic!("expected UnexpectedStopReason, got: {other:?}"),
        }
    }

    /// `run_chat` must return `Err(UnexpectedStopReason(reason))` for any
    /// stop_reason that is neither `"end_turn"` nor `"tool_use"`.
    #[tokio::test]
    async fn run_chat_unknown_stop_reason_returns_error() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        let resp = serde_json::json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "truncated"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run_chat(vec![Message::user_text("hello")], &[], None)
            .await;

        match &result {
            Err(AgentError::UnexpectedStopReason(s)) => {
                assert_eq!(
                    s, "max_tokens",
                    "error must contain the original stop_reason; got: {s}"
                );
            }
            other => panic!("expected UnexpectedStopReason(\"max_tokens\"), got: {other:?}"),
        }
    }

    // ── end_turn Resolved write edge cases ────────────────────────────────────

    /// When `update_promise` hangs during the `end_turn` Resolved write,
    /// `NATS_KV_TIMEOUT` fires, a warning is logged, and the run still returns
    /// `Ok(text)`.  The promise stays `Running` — the terminal write was never
    /// committed.
    ///
    /// Distinct from `update_promise_kv_timeout_disables_checkpointing_and_run_completes`:
    /// that test times out a checkpoint write in the tool_use branch, which
    /// sets `checkpoint = None`.  Here `checkpoint` is still `Some` when the
    /// end_turn Resolved write hangs, exercising the `Err(_)` arm directly.
    #[tokio::test(start_paused = true)]
    async fn end_turn_resolved_write_timeout_run_still_completes() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::HangingUpdateStore;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(HangingUpdateStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        // Direct end_turn with no preceding tool_use.  The only update_promise
        // call is the Resolved write inside end_turn.
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "finished"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + std::time::Duration::from_millis(1)),
        );

        assert_eq!(
            result.unwrap(),
            "finished",
            "run must return Ok(text) even when the end_turn Resolved write times out"
        );

        // update_promise timed out — promise status is still Running.
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            crate::promise_store::PromiseStatus::Running,
            "promise must remain Running when the end_turn Resolved write times out"
        );
    }

    /// When `update_promise` returns a CAS conflict during the `end_turn`
    /// Resolved write, the code reloads the current revision and retries once.
    /// `CasConflictOnceStore` only conflicts on the first call, so the retry
    /// succeeds and the promise is ultimately marked Resolved.
    ///
    /// Uses `CasConflictOnceStore` with a direct end_turn (no tool_use) so
    /// the only checkpoint update_promise calls are the Resolved write + retry.
    #[tokio::test]
    async fn end_turn_resolved_write_error_run_still_completes() {
        use crate::promise_store::mock::CasConflictOnceStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(CasConflictOnceStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must return Ok(text) even when the first Resolved write CAS-conflicts"
        );

        // CAS conflict on first write → reload + retry → retry succeeds → Resolved.
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after reload+retry when first write CAS-conflicts"
        );
    }

    // ── write_promise_terminal CAS conflict sub-paths ─────────────────────────
    //
    // The existing test `end_turn_resolved_write_error_run_still_completes`
    // covers the happy-retry path (CAS conflict → reload → retry succeeds).
    // The five tests below cover every other branch inside the
    // `Ok(Err(e))` arm of `write_promise_terminal`:
    //
    //   CAS → reload → None  (promise vanished)
    //   CAS → reload → Err   (reload itself errors)
    //   CAS → reload → hang  (reload times out)
    //   CAS → reload → Some  → retry write → Err
    //   CAS → reload → Some  → retry write → hang  (retry times out)
    //
    // In all five cases the run must still complete with Ok(text), and the
    // promise stays Running (terminal write failed non-fatally).

    /// Shared configurable mock for the five CAS-conflict branch tests.
    ///
    /// `update_promise`:
    ///   - Call 0:  always returns a CAS-conflict Err (enters retry branch)
    ///   - Call 1+: depends on `retry_write`
    ///
    /// `get_promise`:
    ///   - Call 0:  always delegates to inner (initial load in `run()`)
    ///   - Call 1+: depends on `get_after_first`
    #[derive(Clone)]
    enum GetAfterFirstBehavior {
        ReturnNone,
        ReturnErr,
        Hang,
        Delegate,
    }
    #[derive(Clone)]
    enum RetryWriteBehavior {
        ReturnErr,
        Hang,
    }

    struct CasConflictBranchStore {
        inner: crate::promise_store::mock::MockPromiseStore,
        get_count: Arc<std::sync::atomic::AtomicUsize>,
        update_count: Arc<std::sync::atomic::AtomicUsize>,
        get_after_first: GetAfterFirstBehavior,
        retry_write: RetryWriteBehavior,
    }

    impl CasConflictBranchStore {
        fn new(get_after_first: GetAfterFirstBehavior, retry_write: RetryWriteBehavior) -> Self {
            Self {
                inner: crate::promise_store::mock::MockPromiseStore::new(),
                get_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                update_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                get_after_first,
                retry_write,
            }
        }
    }

    impl crate::promise_store::PromiseRepository for CasConflictBranchStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<crate::promise_store::PromiseEntry>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            use std::sync::atomic::Ordering;
            let n = self.get_count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return self.inner.get_promise(tenant_id, promise_id);
            }
            match &self.get_after_first {
                GetAfterFirstBehavior::ReturnNone => Box::pin(async { Ok(None) }),
                GetAfterFirstBehavior::ReturnErr => Box::pin(async {
                    Err(crate::promise_store::PromiseStoreError(
                        "injected reload error".to_string(),
                    ))
                }),
                GetAfterFirstBehavior::Hang => Box::pin(std::future::pending()),
                GetAfterFirstBehavior::Delegate => self.inner.get_promise(tenant_id, promise_id),
            }
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a crate::promise_store::AgentPromise,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a crate::promise_store::AgentPromise,
            revision: u64,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            use std::sync::atomic::Ordering;
            let n = self.update_count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // Call 0 is the pre-LLM heartbeat write — let it succeed so these
                // tests exercise the terminal-write CAS conflict in isolation.
                return self
                    .inner
                    .update_promise(tenant_id, promise_id, promise, revision);
            }
            if n == 1 {
                // Call 1 is the terminal (Resolved) write — inject the CAS conflict.
                return Box::pin(async {
                    Err(crate::promise_store::PromiseStoreError(
                        "injected CAS conflict".to_string(),
                    ))
                });
            }
            // Call 2+ is the retry write after the reload.
            match &self.retry_write {
                RetryWriteBehavior::ReturnErr => Box::pin(async {
                    Err(crate::promise_store::PromiseStoreError(
                        "injected retry write error".to_string(),
                    ))
                }),
                RetryWriteBehavior::Hang => Box::pin(std::future::pending()),
            }
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<String>, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner
                .put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<crate::promise_store::AgentPromise>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.list_running(tenant_id)
        }
    }

    /// CAS conflict on terminal write → reload returns `None` (promise vanished).
    /// Run must still complete with Ok(text); promise status unchanged (Running).
    #[tokio::test]
    async fn write_promise_terminal_cas_conflict_promise_vanished_run_completes() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(CasConflictBranchStore::new(
            GetAfterFirstBehavior::ReturnNone,
            RetryWriteBehavior::ReturnErr, // never reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let resp = serde_json::json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "done"}]});
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));
        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when reload returns None"
        );

        // update_promise failed → promise never written → still Running
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must stay Running when reload returns None"
        );
    }

    /// CAS conflict on terminal write → reload returns `Err`.
    /// Run must still complete; promise stays Running.
    #[tokio::test]
    async fn write_promise_terminal_cas_conflict_reload_error_run_completes() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(CasConflictBranchStore::new(
            GetAfterFirstBehavior::ReturnErr,
            RetryWriteBehavior::ReturnErr, // never reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let resp = serde_json::json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "done"}]});
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));
        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when reload returns Err"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must stay Running when reload errors"
        );
    }

    /// CAS conflict on terminal write → reload hangs past `NATS_KV_TIMEOUT`.
    /// Run must still complete; promise stays Running.
    #[tokio::test(start_paused = true)]
    async fn write_promise_terminal_cas_conflict_reload_timeout_run_completes() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use std::time::Duration;

        let store = Arc::new(CasConflictBranchStore::new(
            GetAfterFirstBehavior::Hang,
            RetryWriteBehavior::ReturnErr, // never reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let resp = serde_json::json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "done"}]});
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));
        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when reload times out"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must stay Running when reload times out"
        );
    }

    /// CAS conflict on terminal write → reload succeeds → retry write returns `Err`.
    /// Run must still complete; promise stays Running.
    #[tokio::test]
    async fn write_promise_terminal_cas_conflict_retry_write_error_run_completes() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(CasConflictBranchStore::new(
            GetAfterFirstBehavior::Delegate, // reload returns the real promise
            RetryWriteBehavior::ReturnErr,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let resp = serde_json::json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "done"}]});
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));
        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when retry write errors"
        );

        // retry write Err → promise status not written by the store (inner never called by update)
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must stay Running when retry write errors"
        );
    }

    /// CAS conflict on terminal write → reload succeeds → retry write hangs past `NATS_KV_TIMEOUT`.
    /// Run must still complete; promise stays Running.
    #[tokio::test(start_paused = true)]
    async fn write_promise_terminal_cas_conflict_retry_write_timeout_run_completes() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use std::time::Duration;

        let store = Arc::new(CasConflictBranchStore::new(
            GetAfterFirstBehavior::Delegate,
            RetryWriteBehavior::Hang,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let resp = serde_json::json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "done"}]});
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));
        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("unused")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when retry write times out"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must stay Running when retry write times out"
        );
    }

    // ── Pre-LLM heartbeat ─────────────────────────────────────────────────────
    //
    // HEARTBEAT_REFRESH_INTERVAL is Duration::ZERO in test builds, so the
    // heartbeat fires on every iteration.  Each test runs one iteration (single
    // end_turn response) and uses HeartbeatTestStore to drive a specific branch.
    //
    // Call sequence for a single end_turn run with a durable agent:
    //   get_promise   call 0  → initial checkpoint load (always delegates)
    //   update_promise call 0 → heartbeat initial write  (HbInitial)
    //   get_promise   call 1  → heartbeat reload after failure (HbReload)
    //   update_promise call 1 → heartbeat retry write    (HbRetry)
    //   update_promise call 2+→ terminal (Resolved) write (delegates to inner)

    #[derive(Clone)]
    enum HbInitial {
        Succeed,
        CasConflict,
        Hang,
    }
    #[derive(Clone)]
    enum HbReload {
        None_,
        NonRunning,
        Err_,
        HangTimeout,
        Running,
    }
    #[derive(Clone)]
    enum HbRetry {
        Succeed,
        Err_,
        Hang,
    }

    struct HeartbeatTestStore {
        inner: crate::promise_store::mock::MockPromiseStore,
        get_count: Arc<std::sync::atomic::AtomicUsize>,
        update_count: Arc<std::sync::atomic::AtomicUsize>,
        hb_initial: HbInitial,
        hb_reload: HbReload,
        hb_retry: HbRetry,
    }

    impl HeartbeatTestStore {
        fn new(hb_initial: HbInitial, hb_reload: HbReload, hb_retry: HbRetry) -> Self {
            Self {
                inner: crate::promise_store::mock::MockPromiseStore::new(),
                get_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                update_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                hb_initial,
                hb_reload,
                hb_retry,
            }
        }
    }

    impl crate::promise_store::PromiseRepository for HeartbeatTestStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<crate::promise_store::PromiseEntry>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            use std::sync::atomic::Ordering;
            let n = self.get_count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return self.inner.get_promise(tenant_id, promise_id);
            }
            if n == 1 {
                return match &self.hb_reload {
                    HbReload::None_ => Box::pin(async { Ok(None) }),
                    HbReload::NonRunning => {
                        let pid = promise_id.to_string();
                        Box::pin(async move {
                            let p = crate::promise_store::AgentPromise {
                                id: pid,
                                tenant_id: "acme".to_string(),
                                automation_id: String::new(),
                                status: crate::promise_store::PromiseStatus::Resolved,
                                messages: vec![],
                                iteration: 0,
                                worker_id: "test-worker".to_string(),
                                claimed_at: 0,
                                trigger: serde_json::Value::Null,
                                nats_subject: "test.subject".to_string(),
                                system_prompt: None,
                                recovery_count: 0,
                                checkpoint_degraded: false,
                                failure_reason: None,
                            };
                            Ok(Some((p, 99)))
                        })
                    }
                    HbReload::Err_ => Box::pin(async {
                        Err(crate::promise_store::PromiseStoreError(
                            "injected heartbeat reload error".to_string(),
                        ))
                    }),
                    HbReload::HangTimeout => Box::pin(std::future::pending()),
                    HbReload::Running => self.inner.get_promise(tenant_id, promise_id),
                };
            }
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a crate::promise_store::AgentPromise,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a crate::promise_store::AgentPromise,
            revision: u64,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            use std::sync::atomic::Ordering;
            let n = self.update_count.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => match &self.hb_initial {
                    HbInitial::Succeed => self
                        .inner
                        .update_promise(tenant_id, promise_id, promise, revision),
                    HbInitial::CasConflict => Box::pin(async {
                        Err(crate::promise_store::PromiseStoreError(
                            "injected heartbeat CAS conflict".to_string(),
                        ))
                    }),
                    HbInitial::Hang => Box::pin(std::future::pending()),
                },
                1 => match &self.hb_retry {
                    HbRetry::Succeed => self
                        .inner
                        .update_promise(tenant_id, promise_id, promise, revision),
                    HbRetry::Err_ => Box::pin(async {
                        Err(crate::promise_store::PromiseStoreError(
                            "injected heartbeat retry error".to_string(),
                        ))
                    }),
                    HbRetry::Hang => Box::pin(std::future::pending()),
                },
                // call 2+ = terminal (Resolved) write and any retries — always delegate
                _ => self
                    .inner
                    .update_promise(tenant_id, promise_id, promise, revision),
            }
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<String>, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner
                .put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<crate::promise_store::AgentPromise>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.list_running(tenant_id)
        }
    }

    /// Helper: build a durable agent backed by a `HeartbeatTestStore`.
    fn make_hb_agent(
        anthropic: Arc<dyn AnthropicClient>,
        store: Arc<HeartbeatTestStore>,
    ) -> AgentLoop {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::PromiseRepository;
        use crate::tools::ToolContext;

        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(crate::tools::mock::MockToolDispatcher::new("unused")),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p1".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        }
    }

    fn end_turn_anthropic() -> Arc<mock::SequencedMockAnthropicClient> {
        Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            serde_json::json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "done"}]}),
        ]))
    }

    /// Happy path: heartbeat write succeeds on the first try — run completes and
    /// the promise is marked `Resolved`.
    #[tokio::test]
    async fn pre_llm_heartbeat_success_updates_rev_and_run_completes() {
        use crate::promise_store::PromiseStatus;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::Succeed,
            HbReload::None_,  // not reached
            HbRetry::Succeed, // not reached (call 1 = terminal write, delegates)
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(result.unwrap(), "done");

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after successful run"
        );
    }

    /// CAS conflict on heartbeat write → reload returns `None` (promise vanished)
    /// → `run()` returns `Ok("")` immediately to avoid duplicate side-effects.
    #[tokio::test]
    async fn pre_llm_heartbeat_cas_conflict_reload_none_stops_run() {
        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::CasConflict,
            HbReload::None_,
            HbRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "",
            "run must stop when promise vanishes during heartbeat reload"
        );
    }

    /// CAS conflict on heartbeat write → reload returns a non-`Running` status
    /// → `run()` returns `Ok("")` to avoid racing with the other worker.
    #[tokio::test]
    async fn pre_llm_heartbeat_cas_conflict_reload_non_running_stops_run() {
        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::CasConflict,
            HbReload::NonRunning,
            HbRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "",
            "run must stop when promise is no longer Running during heartbeat"
        );
    }

    /// CAS conflict on heartbeat write → reload returns `Err` → run continues
    /// (claimed_at may be stale but that is non-fatal).
    #[tokio::test]
    async fn pre_llm_heartbeat_cas_conflict_reload_error_run_continues() {
        use crate::promise_store::PromiseStatus;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::CasConflict,
            HbReload::Err_,
            HbRetry::Succeed, // call 1 = terminal write, delegates to inner
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when heartbeat reload errors"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after run completes"
        );
    }

    /// CAS conflict on heartbeat write → reload returns `Running` → retry write
    /// succeeds → run completes normally.
    #[tokio::test]
    async fn pre_llm_heartbeat_cas_conflict_retry_success_run_completes() {
        use crate::promise_store::PromiseStatus;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::CasConflict,
            HbReload::Running,
            HbRetry::Succeed,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete after successful heartbeat retry"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after run completes"
        );
    }

    /// CAS conflict on heartbeat write → reload returns `Running` → retry write
    /// returns `Err` → run continues (non-fatal).
    #[tokio::test]
    async fn pre_llm_heartbeat_cas_conflict_retry_error_run_continues() {
        use crate::promise_store::PromiseStatus;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::CasConflict,
            HbReload::Running,
            HbRetry::Err_,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when heartbeat retry write errors"
        );

        // Retry write Err → rev not updated; terminal write at call 2 delegates
        // to inner and uses the original rev — should still succeed.
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after run completes"
        );
    }

    /// Initial heartbeat write times out → `NATS_KV_TIMEOUT` fires → reload
    /// returns `None` → `run()` returns `Ok("")` immediately.
    #[tokio::test(start_paused = true)]
    async fn pre_llm_heartbeat_initial_timeout_reload_none_stops_run() {
        use std::time::Duration;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::Hang,
            HbReload::None_,
            HbRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "",
            "run must stop when promise vanishes after heartbeat timeout"
        );
    }

    /// Initial heartbeat write times out → reload returns a non-`Running` status
    /// → `run()` returns `Ok("")`.
    #[tokio::test(start_paused = true)]
    async fn pre_llm_heartbeat_initial_timeout_reload_non_running_stops_run() {
        use std::time::Duration;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::Hang,
            HbReload::NonRunning,
            HbRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "",
            "run must stop when promise is no longer Running after heartbeat timeout"
        );
    }

    /// CAS conflict on heartbeat write → reload hangs past `NATS_KV_TIMEOUT` →
    /// run continues (claimed_at may be stale, non-fatal).
    #[tokio::test(start_paused = true)]
    async fn pre_llm_heartbeat_cas_conflict_reload_timeout_run_continues() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::CasConflict,
            HbReload::HangTimeout,
            HbRetry::Succeed, // not reached; call 1 = terminal write delegates
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when heartbeat reload times out"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after run completes"
        );
    }

    /// CAS conflict on heartbeat write → reload returns `Running` → retry write
    /// hangs past `NATS_KV_TIMEOUT` → run continues (non-fatal).
    #[tokio::test(start_paused = true)]
    async fn pre_llm_heartbeat_cas_conflict_retry_timeout_run_continues() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::CasConflict,
            HbReload::Running,
            HbRetry::Hang,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when heartbeat retry write times out"
        );

        // Retry write timed out → rev not updated from reload; terminal write at
        // call 2 delegates to inner using original rev — should still succeed.
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after run completes"
        );
    }

    /// Initial heartbeat write times out → reload returns `Running` → retry write
    /// succeeds → run completes normally.
    #[tokio::test(start_paused = true)]
    async fn pre_llm_heartbeat_initial_timeout_reload_succeeds_run_completes() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(HeartbeatTestStore::new(
            HbInitial::Hang,
            HbReload::Running,
            HbRetry::Succeed,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_hb_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete after heartbeat timeout → reload → retry"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after run completes"
        );
    }

    // ── end_turn multi-block text join ────────────────────────────────────────

    /// When `end_turn` content contains multiple `Text` blocks they are joined
    /// with `"\n"`.  All existing tests pass a single text block; this covers
    /// the join path in `run()`.
    #[tokio::test]
    async fn run_end_turn_joins_multiple_text_blocks_with_newline() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        let resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [
                {"type": "text", "text": "first"},
                {"type": "text", "text": "second"}
            ]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        assert_eq!(
            result, "first\nsecond",
            "run() must join multiple text blocks with '\\n'"
        );
    }

    /// Same as above but for `run_chat()`, which has its own independent
    /// text-collection loop.
    #[tokio::test]
    async fn run_chat_end_turn_joins_multiple_text_blocks_with_newline() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        let resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [
                {"type": "text", "text": "line one"},
                {"type": "text", "text": "line two"}
            ]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let (text, _) = agent
            .run_chat(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        assert_eq!(
            text, "line one\nline two",
            "run_chat() must join multiple text blocks with '\\n'"
        );
    }

    // ── non-object tool input — idempotency key not injected ──────────────────

    /// When a built-in tool receives a non-object input (null, array, scalar),
    /// `as_object_mut()` returns `None` and `_idempotency_key` is NOT injected.
    /// The dispatcher receives the original input unchanged.
    #[tokio::test]
    async fn non_object_tool_input_does_not_receive_idempotency_key() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Mutex;

        struct CapturingDispatcher {
            last_input: Arc<Mutex<Option<serde_json::Value>>>,
        }
        impl crate::tools::ToolDispatcher for CapturingDispatcher {
            fn dispatch<'a>(
                &'a self,
                _name: &'a str,
                input: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
                *self.last_input.lock().unwrap() = Some(input.clone());
                Box::pin(async { "ok".to_string() })
            }
        }

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        // input: null — not an object, so as_object_mut() returns None.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": null}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let captured = Arc::new(Mutex::new(None::<serde_json::Value>));
        let dispatcher = Arc::new(CapturingDispatcher {
            last_input: Arc::clone(&captured),
        });

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );
        agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        let input = captured
            .lock()
            .unwrap()
            .take()
            .expect("dispatcher was never called");
        assert!(
            input.get("_idempotency_key").is_none(),
            "_idempotency_key must NOT be injected into non-object tool input; got: {input}"
        );
        assert!(
            input.is_null(),
            "non-object input must be forwarded unchanged; got: {input}"
        );
    }

    // ── system_prompt not overwritten on second checkpoint ────────────────────

    /// After the first checkpoint stores the system_prompt, subsequent tool
    /// turns must NOT overwrite it.  The `if p.system_prompt.is_none()` guard
    /// in the checkpoint write enforces this.
    ///
    /// Two tool turns are used so both the "store on first" and the "skip on
    /// second" branches execute.  The final promise must contain `iteration = 2`
    /// (both turns checkpointed) and the original system prompt.
    #[tokio::test]
    async fn system_prompt_not_overwritten_on_subsequent_checkpoints() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = || {
            serde_json::json!({
                "stop_reason": "tool_use",
                "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
            })
        };
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp(),
            tool_use_resp(),
            end_turn_resp,
        ]));

        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("ok")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let system_prompt = "You are a test assistant.";
        agent
            .run(vec![Message::user_text("go")], &[], Some(system_prompt))
            .await
            .unwrap();

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.system_prompt,
            Some(system_prompt.to_string()),
            "system_prompt must be stored on the first checkpoint and preserved on subsequent ones"
        );
        assert_eq!(
            p.iteration, 2,
            "both tool turns must have been checkpointed (iteration advances once per tool turn)"
        );
    }

    // ── end_turn non-Text block filtering ─────────────────────────────────────

    /// When `end_turn` content contains non-Text blocks (e.g. a ToolUse block),
    /// the `filter_map` in the text-collection loop silently discards them.
    /// Only the text from `Text` blocks is returned.
    #[tokio::test]
    async fn end_turn_non_text_content_blocks_are_filtered_out() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        // end_turn with a ToolUse block first, then a Text block.
        // Result must be "actual response" (ToolUse is discarded).
        let resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [
                {"type": "tool_use", "id": "tu1", "name": "some_tool", "input": {}},
                {"type": "text", "text": "actual response"}
            ]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        assert_eq!(
            result, "actual response",
            "non-Text content blocks in end_turn must be discarded by filter_map"
        );
    }

    // ── sort_json_keys scalar arm ─────────────────────────────────────────────

    /// `sort_json_keys` must return scalar values (null, number, string, bool)
    /// unchanged — the `other => other.clone()` arm.
    #[test]
    fn sort_json_keys_passes_through_scalar_values_unchanged() {
        for scalar in [
            serde_json::Value::Null,
            serde_json::json!(42),
            serde_json::json!("hello"),
            serde_json::json!(true),
        ] {
            assert_eq!(
                sort_json_keys(&scalar),
                scalar,
                "sort_json_keys must return scalar values unchanged; input: {scalar}"
            );
        }
    }

    // ── Message::assistant constructor ────────────────────────────────────────

    /// `Message::assistant` sets `role = "assistant"` and stores the given
    /// content slice.
    #[test]
    fn message_assistant_has_correct_role_and_content() {
        let content = vec![ContentBlock::Text {
            text: "hi".to_string(),
        }];
        let msg = Message::assistant(content);
        assert_eq!(msg.role, "assistant");
        assert_eq!(msg.content.len(), 1);
        assert!(matches!(&msg.content[0], ContentBlock::Text { text } if text == "hi"));
    }

    // ── ContentBlock serde round-trips ────────────────────────────────────────

    /// `ContentBlock::ToolUse` must serialize and deserialize correctly,
    /// including the `"type": "tool_use"` tag required by the Anthropic wire format.
    #[test]
    fn content_block_tool_use_round_trips_json() {
        let block = ContentBlock::ToolUse {
            id: "tu1".to_string(),
            name: "my_tool".to_string(),
            input: serde_json::json!({"key": "val"}),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(
            json.contains("\"type\":\"tool_use\""),
            "serialized ToolUse must contain the type tag; got: {json}"
        );
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, ContentBlock::ToolUse { ref id, ref name, .. }
                if id == "tu1" && name == "my_tool"),
            "ToolUse must survive a serde round-trip"
        );
    }

    /// `ContentBlock::ToolResult` must serialize and deserialize correctly,
    /// including the `"type": "tool_result"` tag.
    #[test]
    fn content_block_tool_result_round_trips_json() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "tu1".to_string(),
            content: "output text".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(
            json.contains("\"type\":\"tool_result\""),
            "serialized ToolResult must contain the type tag; got: {json}"
        );
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, ContentBlock::ToolResult { ref tool_use_id, ref content }
                if tool_use_id == "tu1" && content == "output text"),
            "ToolResult must survive a serde round-trip"
        );
    }

    /// `AnthropicMessageRef` must omit `usage` so Anthropic does not return 400.
    /// `Message` itself serializes `usage` for NATS KV persistence.
    #[test]
    fn anthropic_message_ref_omits_usage() {
        let mut msg = Message::assistant(vec![ContentBlock::Text { text: "hi".into() }]);
        msg.usage = Some(Usage { input_tokens: 10, output_tokens: 5, ..Default::default() });

        let wire = serde_json::to_string(&AnthropicMessageRef(&msg)).unwrap();
        assert!(
            !wire.contains("usage"),
            "AnthropicMessageRef must not serialize `usage` — Anthropic returns 400 for extra fields; got: {wire}"
        );

        let stored = serde_json::to_string(&msg).unwrap();
        assert!(
            stored.contains("usage"),
            "Message must serialize `usage` for NATS KV persistence; got: {stored}"
        );
    }

    // ── tool_cache_key with empty inputs ──────────────────────────────────────

    /// `tool_cache_key` with an empty object and an empty array must each
    /// produce a stable 64-char hex SHA-256 digest, and the two must differ.
    #[test]
    fn tool_cache_key_stable_for_empty_inputs() {
        let key_obj = tool_cache_key("t", &serde_json::json!({}));
        let key_arr = tool_cache_key("t", &serde_json::json!([]));

        assert_eq!(key_obj.len(), 64, "empty-object key must be 64 hex chars");
        assert!(
            key_obj.chars().all(|c| c.is_ascii_hexdigit()),
            "empty-object key must be lowercase hex"
        );

        assert_eq!(key_arr.len(), 64, "empty-array key must be 64 hex chars");
        assert!(
            key_arr.chars().all(|c| c.is_ascii_hexdigit()),
            "empty-array key must be lowercase hex"
        );

        // Deterministic: same input → same key.
        assert_eq!(key_obj, tool_cache_key("t", &serde_json::json!({})));
        assert_eq!(key_arr, tool_cache_key("t", &serde_json::json!([])));

        // Empty object and empty array must produce distinct keys.
        assert_ne!(
            key_obj, key_arr,
            "empty object and empty array must produce different cache keys"
        );
    }

    // ── promise_store Some with promise_id None ───────────────────────────────

    /// When `promise_store` is `Some` but `promise_id` is `None`, every
    /// `if let (Some(store), Some(pid))` guard fails and the run completes
    /// normally without any KV reads or writes.
    #[tokio::test]
    async fn promise_store_some_without_promise_id_skips_all_checkpointing() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        let store = Arc::new(MockPromiseStore::new());
        // No promise inserted — if the run tried to load one it would get None,
        // but the real check is that it completes without any write activity.

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: None, // paired guard requires both — None disables all checkpointing
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        assert_eq!(result, "done");
        assert!(
            store.snapshot_promises().is_empty(),
            "no KV writes must occur when promise_id is None even if promise_store is Some"
        );
    }

    /// When `promise_store = Some(...)` but `promise_id = None`, the tool
    /// cache guard and idempotency key guard both require `promise_id` — both
    /// are skipped. The tool must execute normally without `_idempotency_key`
    /// injection and no KV writes must occur, mirroring the fully non-durable
    /// case tested by `non_durable_tool_use_executes_without_idempotency_key`.
    #[tokio::test]
    async fn promise_id_none_with_promise_store_tool_use_skips_cache_and_idempotency() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::{ToolContext, ToolDispatcher};

        struct CapturingDispatcher {
            captured: std::sync::Mutex<Vec<serde_json::Value>>,
        }
        impl ToolDispatcher for CapturingDispatcher {
            fn dispatch<'a>(
                &'a self,
                _name: &'a str,
                input: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
                self.captured.lock().unwrap().push(input.clone());
                Box::pin(async { "ok".to_string() })
            }
        }

        let store = Arc::new(MockPromiseStore::new());

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {"x": 1}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let capturing = Arc::new(CapturingDispatcher {
            captured: std::sync::Mutex::new(vec![]),
        });

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::clone(&capturing) as Arc<dyn ToolDispatcher>,
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();
        assert_eq!(result, "done");

        let inputs = capturing.captured.lock().unwrap();
        assert_eq!(inputs.len(), 1, "tool must be dispatched exactly once");
        assert!(
            inputs[0].get("_idempotency_key").is_none(),
            "no _idempotency_key when promise_id is None even with promise_store wired; got: {}",
            inputs[0]
        );
        assert!(
            store.snapshot_promises().is_empty(),
            "no KV writes must occur when promise_id is None even if promise_store is Some"
        );
    }

    // ── max_iterations = 0 ───────────────────────────────────────────────────

    /// When `max_iterations = 0` the for-loop body never runs — no Anthropic
    /// call is made.  The agent must write `PermanentFailed` to KV via the
    /// post-loop `write_promise_terminal` call and return
    /// `Err(MaxIterationsReached)`.
    #[tokio::test]
    async fn max_iterations_zero_with_promise_store_fails_immediately() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::{ToolContext, mock::MockToolDispatcher};
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        // Empty queue — panics if Anthropic is called at all.
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 0,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p1".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;

        assert!(
            matches!(result, Err(AgentError::MaxIterationsReached)),
            "max_iterations=0 must return MaxIterationsReached"
        );

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "promise must be marked PermanentFailed when max_iterations=0"
        );
    }

    /// When `max_iterations = 0` and `promise_store = None` (non-durable run),
    /// the loop body never executes, the post-loop KV guard is skipped, and
    /// `run()` returns `Err(MaxIterationsReached)` without touching any store.
    ///
    /// Symmetric counterpart to `max_iterations_zero_with_promise_store_fails_immediately`
    /// which covers the durable case.
    #[tokio::test]
    async fn max_iterations_zero_without_promise_store_returns_error() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};
        use mock::SequencedMockAnthropicClient;

        // Empty response queue — panics if Anthropic is ever called.
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 0,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;
        assert!(
            matches!(result, Err(AgentError::MaxIterationsReached)),
            "non-durable max_iterations=0 must return MaxIterationsReached; got: {:?}",
            result
        );
    }

    // ── non-durable run with tool_use ────────────────────────────────────────

    /// When `promise_id` is `None`, the agent dispatches tool calls normally
    /// but does NOT inject `_idempotency_key` into the input (no stable run
    /// ID to derive it from) and does NOT cache the result in KV.
    #[tokio::test]
    async fn non_durable_tool_use_executes_without_idempotency_key() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{ToolContext, ToolDispatcher};

        struct CapturingDispatcher {
            captured: std::sync::Mutex<Vec<serde_json::Value>>,
        }
        impl ToolDispatcher for CapturingDispatcher {
            fn dispatch<'a>(
                &'a self,
                _name: &'a str,
                input: &'a serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
                self.captured.lock().unwrap().push(input.clone());
                Box::pin(async move { "tool output".to_string() })
            }
        }

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {"key": "val"}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        let capturing = Arc::new(CapturingDispatcher {
            captured: std::sync::Mutex::new(vec![]),
        });

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::clone(&capturing) as Arc<dyn ToolDispatcher>,
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        assert_eq!(result, "done");

        let inputs = capturing.captured.lock().unwrap();
        assert_eq!(inputs.len(), 1, "tool must be dispatched exactly once");
        assert!(
            inputs[0].get("_idempotency_key").is_none(),
            "no _idempotency_key must be injected when promise_id is None; got: {}",
            inputs[0]
        );
        assert_eq!(
            inputs[0]["key"],
            serde_json::json!("val"),
            "original input fields must be forwarded unchanged"
        );
    }

    // ── ContentBlock::Text serde round-trip ───────────────────────────────────

    /// `ContentBlock::Text` must serialize with `"type":"text"` (the
    /// `#[serde(tag = "type", rename_all = "snake_case")]` tag on the enum)
    /// and survive a full serde round-trip.
    /// `ToolUse` and `ToolResult` variants have existing tests; this covers
    /// the `Text` variant.
    #[test]
    fn content_block_text_round_trips_json() {
        let block = ContentBlock::Text {
            text: "hello world".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();

        assert!(
            json.contains("\"type\":\"text\""),
            "serialized Text must contain the type tag; got: {json}"
        );
        assert!(
            json.contains("\"text\":\"hello world\""),
            "serialized Text must contain the text field; got: {json}"
        );

        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, ContentBlock::Text { ref text } if text == "hello world"),
            "ContentBlock::Text must survive a serde round-trip"
        );
    }

    // ── run_chat with Some(system_prompt) ────────────────────────────────────

    /// When `run_chat` receives `Some(system_prompt)`, the request body sent to
    /// Anthropic must contain a `system` array whose first block has
    /// `"type":"text"` and the supplied prompt text.
    #[tokio::test]
    async fn run_chat_with_system_prompt_sends_system_block() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::ToolContext;

        struct RecordingAnthropicClient {
            captured: std::sync::Mutex<Option<serde_json::Value>>,
            response: serde_json::Value,
        }
        impl AnthropicClient for RecordingAnthropicClient {
            fn complete<'a>(
                &'a self,
                body: serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
            {
                *self.captured.lock().unwrap() = Some(body);
                let resp = self.response.clone();
                Box::pin(async move { Ok(resp) })
            }
        }

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "reply"}]
        });
        let recording = Arc::new(RecordingAnthropicClient {
            captured: std::sync::Mutex::new(None),
            response: end_turn_resp,
        });

        let agent = AgentLoop {
            anthropic_client: Arc::clone(&recording) as Arc<dyn AnthropicClient>,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(crate::tools::mock::MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let (text, _) = agent
            .run_chat(
                vec![Message::user_text("hello")],
                &[],
                Some("my system prompt"),
            )
            .await
            .unwrap();

        assert_eq!(text, "reply");

        let body = recording.captured.lock().unwrap().clone().unwrap();
        let system = &body["system"];
        assert!(
            system.is_array(),
            "request must include a system array when system_prompt is Some; got: {body}"
        );
        assert_eq!(
            system[0]["type"], "text",
            "system block must have type=text"
        );
        assert_eq!(
            system[0]["text"], "my system prompt",
            "system block must carry the supplied prompt text"
        );
    }

    // ── run with Some(system_prompt), no checkpoint ───────────────────────────

    /// `AgentLoop::run` with `Some(system_prompt)` and no promise store
    /// (non-recovering path) must include a `system` array in the Anthropic
    /// request body.
    ///
    /// This covers the `effective_prompt = system_prompt.map(|s| s.to_string())`
    /// branch and the subsequent `effective_prompt.as_deref().map(|text| ...)`
    /// system-block construction inside `run` — symmetrical to
    /// `run_chat_with_system_prompt_sends_system_block` for `run_chat`.
    #[tokio::test]
    async fn run_with_system_prompt_sends_system_block() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::ToolContext;

        struct RecordingAnthropicClient {
            captured: std::sync::Mutex<Option<serde_json::Value>>,
            response: serde_json::Value,
        }
        impl AnthropicClient for RecordingAnthropicClient {
            fn complete<'a>(
                &'a self,
                body: serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
            {
                *self.captured.lock().unwrap() = Some(body);
                let resp = self.response.clone();
                Box::pin(async move { Ok(resp) })
            }
        }

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "reply"}]
        });
        let recording = Arc::new(RecordingAnthropicClient {
            captured: std::sync::Mutex::new(None),
            response: end_turn_resp,
        });

        let agent = AgentLoop {
            anthropic_client: Arc::clone(&recording) as Arc<dyn AnthropicClient>,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(crate::tools::mock::MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(
                vec![Message::user_text("hello")],
                &[],
                Some("my system prompt"),
            )
            .await
            .unwrap();

        assert_eq!(result, "reply");

        let body = recording.captured.lock().unwrap().clone().unwrap();
        let system = &body["system"];
        assert!(
            system.is_array(),
            "request must include a system array when system_prompt is Some; got: {body}"
        );
        assert_eq!(
            system[0]["type"], "text",
            "system block must have type=text"
        );
        assert_eq!(
            system[0]["text"], "my system prompt",
            "system block must carry the supplied prompt text"
        );
    }

    // ── sort_json_keys Array arm ──────────────────────────────────────────────

    /// `sort_json_keys` must recurse into Array elements, sorting any nested
    /// object keys while leaving scalars unchanged.
    ///
    /// This exercises the `Value::Array(arr) => ...` arm directly — not via
    /// `tool_cache_key`. The existing `sort_json_keys_passes_through_scalar_values_unchanged`
    /// test covers scalars; this covers the array arm.
    #[test]
    fn sort_json_keys_array_arm_recurses_into_inner_objects() {
        // Mixed array: an unsorted-key object followed by a scalar.
        let input = serde_json::json!([{"b": 2, "a": 1}, 42]);
        let output = sort_json_keys(&input);

        // The outer array must be preserved.
        let arr = output.as_array().expect("result must be an array");
        assert_eq!(arr.len(), 2, "array length must be unchanged");

        // Inner object keys must be sorted alphabetically.
        let inner = arr[0].as_object().expect("first element must be an object");
        let keys: Vec<&str> = inner.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["a", "b"], "inner object keys must be sorted");
        assert_eq!(arr[0]["a"], 1);
        assert_eq!(arr[0]["b"], 2);

        // Scalar element must pass through unchanged.
        assert_eq!(arr[1], serde_json::json!(42));

        // Empty array must round-trip unchanged.
        assert_eq!(
            sort_json_keys(&serde_json::json!([])),
            serde_json::json!([])
        );
    }

    // ── AgentError::Http display ──────────────────────────────────────────────

    /// `AgentError::Http` must format as `"HTTP error: ..."`.
    ///
    /// `agent_error_display` covers `MaxIterationsReached` and
    /// `UnexpectedStopReason`; `agent_error_source_for_http_variant` builds the
    /// same error but only calls `.source()`. This test covers the `Display` impl
    /// for the `Http` variant.
    #[test]
    fn agent_error_http_display_starts_with_http_error() {
        let display = AgentError::Http(AnthropicClientError::Transient("connection refused".into())).to_string();
        assert!(
            display.starts_with("HTTP error:"),
            "AgentError::Http display must start with 'HTTP error:'; got: {display}"
        );
    }

    // ── run with promise_store=Some but promise not found ─────────────────────

    /// When `promise_store` and `promise_id` are both `Some` but `get_promise`
    /// returns `Ok(None)` (key genuinely absent from KV), `checkpoint` stays
    /// `None` and the run completes normally without any KV writes.
    ///
    /// Distinct from `initial_get_promise_timeout_starts_fresh` (timeout → Err)
    /// and `initial_get_promise_error_starts_fresh` (I/O error → Err). This
    /// exercises the `Ok(None)` arm on line ~614 of `run`.
    #[tokio::test]
    async fn run_promise_not_found_starts_fresh_without_checkpoint_writes() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::{ToolContext, mock::MockToolDispatcher};

        // Empty store — get_promise("acme", "p1") returns Ok(None).
        let store = Arc::new(MockPromiseStore::new());

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "fresh start"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("unused")),
            tool_context: Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", "")),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p1".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await
            .unwrap();

        assert_eq!(result, "fresh start");
        assert!(
            store.snapshot_promises().is_empty(),
            "checkpoint = None when promise not found — no KV writes must occur"
        );
    }

    // ── checkpoint=None at error time ─────────────────────────────────────────

    /// When `checkpoint` is `None` (cleared by a failed CAS reload) and the
    /// next Anthropic call returns an HTTP error, the `write_promise_terminal`
    /// guard silently fails (the `if let … && let Some(checkpoint)` is false)
    /// and the run returns `Err(AgentError::Http)` without writing any terminal
    /// status — the promise stays `Running`.
    ///
    /// This is distinct from `http_4xx_error_marks_promise_permanent_failed`
    /// where `checkpoint` is always `Some` when the error fires.
    #[tokio::test]
    async fn http_error_with_null_checkpoint_does_not_write_terminal_status() {
        use crate::promise_store::mock::DisappearedOnCasReloadStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use std::sync::atomic::{AtomicU32, Ordering};

        // Client: first call → tool_use (triggers CAS → checkpoint cleared);
        //         second call → HTTP error (exercises the null-checkpoint path).
        struct FirstOkThenErrorClient {
            first_response: serde_json::Value,
            calls: AtomicU32,
        }
        impl AnthropicClient for FirstOkThenErrorClient {
            fn complete<'a>(
                &'a self,
                _body: serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
            {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    let resp = self.first_response.clone();
                    Box::pin(async move { Ok(resp) })
                } else {
                    Box::pin(async move { Err(AnthropicClientError::Transient("simulated error".into())) })
                }
            }
        }

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let anthropic = Arc::new(FirstOkThenErrorClient {
            first_response: tool_use_resp,
            calls: AtomicU32::new(0),
        });

        // DisappearedOnCasReloadStore: first update_promise → CAS conflict;
        // subsequent get_promise → Ok(None) → agent_loop sets checkpoint=None.
        let store = Arc::new(DisappearedOnCasReloadStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("ok")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![Message::user_text("go")], &[], None).await;

        // Run must return an HTTP error, not panic.
        assert!(
            matches!(result, Err(AgentError::Http(_))),
            "must return Err(Http) when Anthropic errors after checkpoint cleared"
        );

        // Promise must still be Running — no terminal write was possible with
        // checkpoint=None.
        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must stay Running — terminal write is skipped when checkpoint=None"
        );
    }

    // ── run_chat does not touch promise_store ────────────────────────────────

    /// `run_chat` has no durable-promise logic — it always passes
    /// `recovering = false` and never reads or writes to the promise store.
    ///
    /// Wire a `MockPromiseStore` into the agent and verify the snapshot is
    /// empty (no promise created, no tool result cached) after a full
    /// `run_chat` that includes a tool turn.
    #[tokio::test]
    async fn run_chat_with_promise_store_wired_does_not_read_or_write_kv() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(MockPromiseStore::new());

        // tool_use then end_turn — exercises the tool dispatch path.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "chat done"}]
        });
        let anthropic = Arc::new(mock::SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));

        // Wire promise_store + promise_id just like a durable run would, but
        // use run_chat instead of run.
        let tool_ctx = Arc::new(crate::tools::ToolContext::for_test(
            "http://127.0.0.1:1",
            "",
            "",
            "",
        ));
        let agent = AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("ok")),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(crate::flag_client::AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p-chat".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        };

        let (text, _messages) = agent
            .run_chat(vec![Message::user_text("hello")], &[], None)
            .await
            .unwrap();

        assert_eq!(text, "chat done");
        assert!(
            store.snapshot_promises().is_empty(),
            "run_chat must not create or update any promise in KV"
        );
    }

    // ── ReqwestAnthropicClient retry helpers ──────────────────────────────────

    /// Start a minimal raw-TCP HTTP/1.1 server.  Each incoming connection is
    /// served the next `Vec<u8>` from `responses` in FIFO order.
    /// Returns `(base_url, server_task)` — keep the task alive for the test.
    async fn start_response_server(
        responses: Vec<Vec<u8>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use std::collections::VecDeque;
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let queue: Arc<Mutex<VecDeque<Vec<u8>>>> =
            Arc::new(Mutex::new(responses.into_iter().collect()));

        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut conn, _)) = listener.accept().await else {
                    return;
                };
                let queue = Arc::clone(&queue);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = conn.read(&mut buf).await; // drain request
                    let response = queue
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("response queue exhausted in test");
                    let _ = conn.write_all(&response).await;
                });
            }
        });

        (format!("http://{addr}"), handle)
    }

    fn raw_500() -> Vec<u8> {
        b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            .to_vec()
    }

    fn raw_429_retry_after_1() -> Vec<u8> {
        b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 1\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            .to_vec()
    }

    fn raw_200_json(body: serde_json::Value) -> Vec<u8> {
        let s = body.to_string();
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            s.len(),
            s
        )
        .into_bytes()
    }

    // ── ReqwestAnthropicClient::complete retry tests ──────────────────────────

    /// All four attempts hit ECONNREFUSED — the retry loop exhausts attempts=3
    /// and returns the final connect error.
    ///
    /// Backoff sequence: 2 s → 4 s → 8 s.  Uses Tokio's mock clock so the
    /// test finishes instantly.
    #[tokio::test(start_paused = true)]
    async fn reqwest_client_exhausts_retries_on_connect_error() {
        use std::time::Duration;

        // Bind then immediately drop — reliably produces ECONNREFUSED on Linux.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let client = ReqwestAnthropicClient::new(
            reqwest::Client::new(),
            format!("http://{addr}"),
            "test-token".to_string(),
        );

        let (result, _) = tokio::join!(
            client.complete(serde_json::json!({"model": "test", "max_tokens": 1})),
            async {
                tokio::time::advance(Duration::from_secs(3)).await; // wakes sleep(2s)
                tokio::time::advance(Duration::from_secs(5)).await; // wakes sleep(4s)
                tokio::time::advance(Duration::from_secs(9)).await; // wakes sleep(8s)
            }
        );

        assert!(
            matches!(result.unwrap_err(), AnthropicClientError::Transient(_)),
            "connect error after exhausted retries must be Transient"
        );
    }

    /// One 500 followed by a 200 — the client retries once and succeeds.
    ///
    /// The 2-second backoff sleep after the 500 is advanced by the mock clock.
    #[tokio::test(start_paused = true)]
    async fn reqwest_client_retries_5xx_once_then_succeeds() {
        use std::time::Duration;

        let ok_body = serde_json::json!({"stop_reason": "end_turn", "content": []});
        let (base_url, _server) =
            start_response_server(vec![raw_500(), raw_200_json(ok_body.clone())]).await;

        let client =
            ReqwestAnthropicClient::new(reqwest::Client::new(), base_url, "test-token".to_string());

        let (result, _) = tokio::join!(
            client.complete(serde_json::json!({"model": "test", "max_tokens": 1})),
            async {
                tokio::time::advance(Duration::from_secs(3)).await; // wakes sleep(2s) after 500
            }
        );

        assert_eq!(
            result.unwrap(),
            ok_body,
            "must return the 200 response after one retry"
        );
    }

    /// Four consecutive 500s — the retry loop exhausts all three retries and
    /// returns the final server-error.
    #[tokio::test(start_paused = true)]
    async fn reqwest_client_exhausts_retries_on_5xx() {
        use std::time::Duration;

        let (base_url, _server) =
            start_response_server(vec![raw_500(), raw_500(), raw_500(), raw_500()]).await;

        let client =
            ReqwestAnthropicClient::new(reqwest::Client::new(), base_url, "test-token".to_string());

        let (result, _) = tokio::join!(
            client.complete(serde_json::json!({"model": "test", "max_tokens": 1})),
            async {
                tokio::time::advance(Duration::from_secs(3)).await; // wakes sleep(2s)
                tokio::time::advance(Duration::from_secs(5)).await; // wakes sleep(4s)
                tokio::time::advance(Duration::from_secs(9)).await; // wakes sleep(8s)
            }
        );

        let err = result.expect_err("must fail after exhausting retries on 5xx");
        assert!(
            matches!(err, AnthropicClientError::Transient(_)),
            "5xx error after exhausted retries must be Transient; got: {err}"
        );
    }

    /// One 429 (Retry-After: 1) followed by a 200 — the client waits 1 s and
    /// succeeds on the second attempt.
    #[tokio::test(start_paused = true)]
    async fn reqwest_client_retries_429_once_then_succeeds() {
        use std::time::Duration;

        let ok_body = serde_json::json!({"stop_reason": "end_turn", "content": []});
        let (base_url, _server) =
            start_response_server(vec![raw_429_retry_after_1(), raw_200_json(ok_body.clone())])
                .await;

        let client =
            ReqwestAnthropicClient::new(reqwest::Client::new(), base_url, "test-token".to_string());

        let (result, _) = tokio::join!(
            client.complete(serde_json::json!({"model": "test", "max_tokens": 1})),
            async {
                tokio::time::advance(Duration::from_secs(2)).await; // wakes sleep(1s) from Retry-After
            }
        );

        assert_eq!(
            result.unwrap(),
            ok_body,
            "must return the 200 response after retrying on 429"
        );
    }

    /// Four consecutive 429s (Retry-After: 1) — the retry loop exhausts all
    /// three retries and returns the final 429 error.
    #[tokio::test(start_paused = true)]
    async fn reqwest_client_exhausts_retries_on_429() {
        use std::time::Duration;

        let (base_url, _server) = start_response_server(vec![
            raw_429_retry_after_1(),
            raw_429_retry_after_1(),
            raw_429_retry_after_1(),
            raw_429_retry_after_1(),
        ])
        .await;

        let client =
            ReqwestAnthropicClient::new(reqwest::Client::new(), base_url, "test-token".to_string());

        let (result, _) = tokio::join!(
            client.complete(serde_json::json!({"model": "test", "max_tokens": 1})),
            async {
                tokio::time::advance(Duration::from_secs(2)).await; // wakes sleep(1s) after 429 #1
                tokio::time::advance(Duration::from_secs(2)).await; // wakes sleep(1s) after 429 #2
                tokio::time::advance(Duration::from_secs(2)).await; // wakes sleep(1s) after 429 #3
            }
        );

        let err = result.expect_err("must fail after exhausting 429 retries");
        assert!(
            matches!(err, AnthropicClientError::Transient(_)),
            "429 error after exhausted retries must be Transient; got: {err}"
        );
    }

    /// When a checkpoint exists but its `messages` vec is empty (`recovering =
    /// false`), `effective_prompt` must come from the caller-supplied
    /// `system_prompt`, NOT from the `system_prompt` stored in the checkpoint.
    ///
    /// Verifies the `else { system_prompt.map(...) }` branch: even if the
    /// checkpoint has `system_prompt = Some("stored")`, a non-recovering run
    /// must ignore it and use the freshly-fetched caller value instead.
    #[tokio::test]
    async fn non_recovering_checkpoint_with_system_prompt_uses_caller_prompt() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::MockToolDispatcher;
        use std::sync::Mutex;

        struct RecordingClient {
            bodies: Arc<Mutex<Vec<serde_json::Value>>>,
        }
        impl AnthropicClient for RecordingClient {
            fn complete<'a>(
                &'a self,
                body: serde_json::Value,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<serde_json::Value, AnthropicClientError>>
                        + Send
                        + 'a,
                >,
            > {
                self.bodies.lock().unwrap().push(body);
                Box::pin(async move {
                    Ok(serde_json::json!({
                        "stop_reason": "end_turn",
                        "content": [{"type": "text", "text": "ok"}]
                    }))
                })
            }
        }

        let store = Arc::new(MockPromiseStore::new());

        // Checkpoint: system_prompt is stored, but messages are EMPTY.
        // recovering = false (no messages) → effective_prompt must use caller's.
        let mut p = make_test_promise("p-sys-caller");
        p.messages = vec![];
        p.system_prompt = Some("stored checkpoint prompt".to_string());
        store.insert_promise(p);

        let bodies: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(vec![]));
        let agent = make_durable_agent(
            Arc::new(RecordingClient {
                bodies: Arc::clone(&bodies),
            }),
            Arc::new(MockToolDispatcher::new("ok")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p-sys-caller",
        );

        agent
            .run(vec![Message::user_text("go")], &[], Some("caller prompt"))
            .await
            .unwrap();

        let recorded = bodies.lock().unwrap();
        assert_eq!(recorded.len(), 1, "exactly one LLM call");
        let system_text = recorded[0]["system"][0]["text"]
            .as_str()
            .expect("system[0].text must be present");
        assert_eq!(
            system_text, "caller prompt",
            "non-recovering run must use caller-supplied prompt even when checkpoint has a stored system_prompt"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════
    // Real NATS KV integration tests (agent_loop)
    //
    // These tests exercise AgentLoop::run against a real NATS JetStream
    // container so that NATS KV's actual serialization, size limits, and CAS
    // semantics are exercised — not an in-memory mock.
    // ══════════════════════════════════════════════════════════════════════════

    /// Start a throwaway NATS container and open a real `PromiseStore`.
    /// Returns the store and an opaque drop guard — keep the guard alive for
    /// the duration of the test or the container will be stopped.
    async fn make_real_promise_store() -> (Arc<crate::promise_store::PromiseStore>, impl Drop) {
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
        let ps = Arc::new(
            crate::promise_store::PromiseStore::open(&js)
                .await
                .expect("PromiseStore::open"),
        );
        (ps, container)
    }

    /// The oversized-checkpoint guard must fire against real NATS KV: when
    /// staging the accumulated messages into an `AgentPromise` would push the
    /// serialized size over `CHECKPOINT_MAX_BYTES`, the run must:
    ///   1. Complete successfully (`Ok("done")`).
    ///   2. Skip the message-history write — `iteration` stays 0, `messages`
    ///      stays empty in real KV.
    ///   3. Still write the terminal `Resolved` status — the checkpoint
    ///      revision is kept alive so the terminal write can succeed.
    ///
    /// This is the real-KV counterpart of
    /// `oversized_checkpoint_disables_checkpointing_but_run_completes`.
    /// The key difference: here the guard is tested against an actual NATS KV
    /// bucket, confirming no oversized payload is ever submitted.
    #[tokio::test]
    async fn real_kv_oversized_checkpoint_guard_prevents_nats_write() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let (ps, _c) = make_real_promise_store().await;
        let store: Arc<dyn PromiseRepository> = ps.clone();
        store
            .put_promise(&make_test_promise("p-oversize"))
            .await
            .unwrap();

        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("tool result"));
        let agent = make_durable_agent(anthropic, dispatcher, Arc::clone(&store), "p-oversize");

        // Initial messages large enough that staging them into the AgentPromise
        // pushes the total serialized size above CHECKPOINT_MAX_BYTES.
        let large_text = "a".repeat(CHECKPOINT_MAX_BYTES + 1);
        let result = agent
            .run(vec![Message::user_text(large_text)], &[], None)
            .await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete normally even when checkpoint payload is oversized"
        );

        let (p, _) = ps
            .get_promise("acme", "p-oversize")
            .await
            .unwrap()
            .expect("promise must still exist in real KV");
        assert!(
            p.messages.is_empty(),
            "oversized message history must not be written to real NATS KV"
        );
        assert_eq!(
            p.iteration, 0,
            "iteration must not advance when the size guard disables checkpointing"
        );
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "terminal Resolved write must succeed even when message-history checkpointing is disabled; got {:?}",
            p.status
        );
    }

    /// After a crash, tool results already stored in the real NATS KV
    /// `AGENT_TOOL_RESULTS` bucket must be replayed without re-dispatching the
    /// tool.
    ///
    /// This is the real-KV counterpart of `recovery_replays_tool_from_cache`.
    /// The difference: the cache round-trips through an actual NATS KV bucket
    /// (serialization, network, bucket isolation) rather than an in-memory map.
    /// In particular, the `tool_use_id` on recovery is different from the one
    /// at crash time — the test uses `"fresh-id-after-restart"` — confirming
    /// the content-based cache key (`SHA-256(name:input)`) survives the restart.
    #[tokio::test]
    async fn real_kv_tool_result_cache_prevents_reexecution_on_recovery() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::CountingMockToolDispatcher;
        use mock::SequencedMockAnthropicClient;
        use std::sync::atomic::Ordering;

        let (ps, _c) = make_real_promise_store().await;
        let store: Arc<dyn PromiseRepository> = ps.clone();

        // Non-empty checkpoint messages → `recovering = true` inside `run()`.
        let mut p = make_test_promise("p-tool-cache");
        p.messages = vec![Message::user_text("start")];
        store.put_promise(&p).await.unwrap();

        // Pre-populate the tool result cache in real NATS KV.
        // The key is the same SHA-256 that `execute_tools` will compute at
        // runtime from the tool name and input — content-based, not id-based.
        let input = serde_json::json!({"k": "v"});
        let ck = super::tool_cache_key("my_tool", &input);
        store
            .put_tool_result("acme", "p-tool-cache", &ck, "cached output")
            .await
            .unwrap();

        // Anthropic returns the same call with a *fresh* tool_use_id — simulating
        // the LLM regenerating the request after a restart.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{
                "type": "tool_use",
                "id": "fresh-id-after-restart",
                "name": "my_tool",
                "input": {"k": "v"}
            }]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "all done"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp,
            end_turn_resp,
        ]));
        let (dispatcher, call_count) =
            CountingMockToolDispatcher::new("live result — must not appear");
        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store),
            "p-tool-cache",
        );

        // Empty initial_messages — run() loads them from the checkpoint.
        let result = agent.run(vec![], &[], None).await;
        assert_eq!(result.unwrap(), "all done");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "tool dispatcher must not be called — result must be replayed from real NATS KV cache"
        );

        let (p, _) = ps
            .get_promise("acme", "p-tool-cache")
            .await
            .unwrap()
            .expect("promise must still exist in real KV");
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved in real KV after recovery; got {:?}",
            p.status
        );
    }

    // ── recovering resets when checkpointing is permanently disabled ──────────

    /// When checkpointing is permanently disabled (payload exceeds all trim
    /// levels), the tool-result cache remains active for subsequent turns because
    /// `execute_tools` gates cache lookups on `promise_id` being set, not on the
    /// `recovering` flag. This means a crash after checkpointing is disabled can
    /// still replay tool results from turns executed after the last successful
    /// checkpoint, rather than re-executing them from scratch.
    ///
    /// Scenario:
    ///  - Checkpoint loaded with a large initial message.
    ///  - Turn 1: my_tool replayed from cache (promise_id set → cache hit).
    ///    Checkpoint oversized (3 messages < MIN_TRIM_KEEP=4) → disabled.
    ///    `checkpoint_degraded = true` written to KV.
    ///  - Turn 2: same my_tool requested again.
    ///    Cache still consulted (promise_id still set) → cache hit → 0 live dispatches.
    ///  - Turn 2 checkpoint attempt: 5 messages, divisor=2 → keep=4 → trim succeeds.
    ///  - Turn 3: end_turn → "done".
    #[tokio::test]
    async fn cache_remains_active_when_checkpointing_permanently_disabled() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::tools::mock::CountingMockToolDispatcher;
        use mock::SequencedMockAnthropicClient;
        use std::sync::atomic::Ordering;

        let store = Arc::new(MockPromiseStore::new());

        // Checkpoint with a message large enough that 3 messages (initial +
        // one tool turn) exceed CHECKPOINT_MAX_BYTES. With only 3 messages
        // the trim logic (MIN_TRIM_KEEP=4) cannot reduce the history, so
        // checkpointing is permanently disabled for turn 1.
        let mut p = make_test_promise("p1");
        p.messages = vec![Message::user_text("a".repeat(CHECKPOINT_MAX_BYTES))];
        store.insert_promise(p);

        // Pre-populate cache for the recovery turn tool call.
        let input = serde_json::json!({"k": "v"});
        let ck = super::tool_cache_key("my_tool", &input);
        store
            .put_tool_result("acme", "p1", &ck, "cached_result")
            .await
            .unwrap();

        let tool_use_resp = || {
            serde_json::json!({
                "stop_reason": "tool_use",
                "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {"k": "v"}}]
            })
        };
        // Turn 2 produces 5 messages total — divisor=2 → keep=4 trims the large
        // initial message. summarize_dropped_messages is called for it.
        let summarize_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "Summary: large initial message."}]
        });
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        // Order: turn-1 LLM, turn-2 LLM, summarize call (turn-2 checkpoint), turn-3 LLM.
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp(),
            tool_use_resp(),
            summarize_resp,
            end_turn_resp,
        ]));
        let (dispatcher, call_count) = CountingMockToolDispatcher::new("live_result");

        let agent = make_durable_agent(
            anthropic,
            Arc::new(dispatcher),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let result = agent.run(vec![], &[], None).await;
        assert_eq!(result.unwrap(), "done");

        // Both turns call my_tool with the same input — both hit the cache.
        // No live dispatch occurs because promise_id is set throughout the run.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "neither turn should dispatch live — cache is active for both turns \
             because promise_id is set, regardless of whether checkpointing is disabled"
        );

        // checkpoint_degraded must be persisted to KV so operators can observe
        // runs that lost durability.
        let (p, _) = store
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert!(
            p.checkpoint_degraded,
            "checkpoint_degraded must be set on the promise when checkpointing is permanently disabled"
        );
    }

    // ── render_messages_for_summary ───────────────────────────────────────────

    #[test]
    fn render_messages_for_summary_empty_slice_returns_empty_string() {
        assert_eq!(render_messages_for_summary(&[]), "");
    }

    #[test]
    fn render_messages_for_summary_text_block() {
        let msgs = vec![Message::user_text("hello world")];
        let out = render_messages_for_summary(&msgs);
        assert_eq!(out, "User: hello world\n\n");
    }

    #[test]
    fn render_messages_for_summary_assistant_text_block() {
        let msgs = vec![Message::assistant(vec![ContentBlock::Text {
            text: "I will help".to_string(),
        }])];
        let out = render_messages_for_summary(&msgs);
        assert_eq!(out, "Assistant: I will help\n\n");
    }

    #[test]
    fn render_messages_for_summary_tool_use_block() {
        let msgs = vec![Message::assistant(vec![ContentBlock::ToolUse {
            id: "tu1".to_string(),
            name: "post_comment".to_string(),
            input: serde_json::json!({"body": "hi"}),
        }])];
        let out = render_messages_for_summary(&msgs);
        assert!(
            out.contains("called tool `post_comment`"),
            "ToolUse must render with backtick-quoted name; got: {out}"
        );
        assert!(
            out.contains("hi"),
            "ToolUse must include the input JSON; got: {out}"
        );
    }

    #[test]
    fn render_messages_for_summary_tool_result_block() {
        let msgs = vec![Message::tool_results(vec![ToolResult {
            tool_use_id: "tu1".to_string(),
            content: "success output".to_string(),
        }])];
        let out = render_messages_for_summary(&msgs);
        assert_eq!(out, "Tool result: success output\n\n");
    }

    #[test]
    fn render_messages_for_summary_unknown_role_passes_through() {
        let msgs = vec![Message {
            role: "system".to_string(),
            content: vec![ContentBlock::Text {
                text: "sys msg".to_string(),
            }],
            usage: None,
        }];
        let out = render_messages_for_summary(&msgs);
        // Unknown role is passed through verbatim.
        assert!(
            out.starts_with("system: "),
            "unknown role must pass through; got: {out}"
        );
    }

    #[test]
    fn render_messages_for_summary_multiple_messages_concatenates() {
        let msgs = vec![
            Message::user_text("first"),
            Message::assistant(vec![ContentBlock::Text {
                text: "second".to_string(),
            }]),
            Message::user_text("third"),
        ];
        let out = render_messages_for_summary(&msgs);
        assert!(out.contains("User: first"), "must contain first user msg");
        assert!(
            out.contains("Assistant: second"),
            "must contain assistant msg"
        );
        assert!(out.contains("User: third"), "must contain third user msg");
        // Each block ends with \n\n — three blocks = three double-newlines.
        assert_eq!(out.matches("\n\n").count(), 3);
    }

    // ── summarize_dropped_messages ────────────────────────────────────────────

    /// When all dropped messages have no content, the rendered string is empty
    /// and the function returns `vec![]` without calling the LLM.
    #[tokio::test]
    async fn summarize_dropped_messages_empty_rendered_returns_empty_vec() {
        // Message with no content blocks → render produces "".
        let msgs = vec![Message {
            role: "user".to_string(),
            content: vec![],
            usage: None,
        }];
        // Empty response queue — panics if LLM is called.
        let client = mock::SequencedMockAnthropicClient::new(vec![]);
        let result = summarize_dropped_messages(&client, "model", &msgs).await;
        assert!(
            result.is_empty(),
            "empty rendered output must return vec![]"
        );
    }

    /// When the LLM call returns an error, the function degrades gracefully
    /// and returns `vec![]` rather than propagating the error.
    #[tokio::test]
    async fn summarize_dropped_messages_llm_error_returns_empty_vec() {
        let msgs = vec![Message::user_text("some content")];
        let client = mock::AlwaysErrAnthropicClient;
        let result = summarize_dropped_messages(&client, "model", &msgs).await;
        assert!(
            result.is_empty(),
            "LLM error must degrade gracefully to vec![]"
        );
    }

    /// When the LLM returns JSON that cannot be deserialized as `AnthropicResponse`,
    /// the function degrades gracefully and returns `vec![]`.
    #[tokio::test]
    async fn summarize_dropped_messages_malformed_llm_response_returns_empty_vec() {
        let msgs = vec![Message::user_text("some content")];
        // Valid JSON but wrong schema — no `stop_reason` or `content` fields.
        let bad_resp = serde_json::json!({"status": "ok", "result": 42});
        let client = mock::SequencedMockAnthropicClient::new(vec![bad_resp]);
        let result = summarize_dropped_messages(&client, "model", &msgs).await;
        assert!(
            result.is_empty(),
            "malformed LLM response must degrade gracefully to vec![]"
        );
    }

    /// When the LLM returns a valid summary text, the function returns exactly
    /// two messages: a user message embedding the summary and an assistant ack.
    #[tokio::test]
    async fn summarize_dropped_messages_valid_response_returns_two_messages() {
        let msgs = vec![Message::user_text("do some work")];
        let summary_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "The agent fetched issue ISS-1 and posted a comment."}]
        });
        let client = mock::SequencedMockAnthropicClient::new(vec![summary_resp]);
        let result = summarize_dropped_messages(&client, "model", &msgs).await;
        assert_eq!(
            result.len(),
            2,
            "must return exactly [user_summary, assistant_ack]"
        );
        // First message: user role with summary text embedded.
        assert_eq!(result[0].role, "user");
        let first_text = match &result[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            other => panic!("expected Text block, got: {other:?}"),
        };
        assert!(
            first_text.contains("ISS-1"),
            "user summary must embed LLM output; got: {first_text}"
        );
        assert!(
            first_text.contains("Context summary"),
            "user summary must include the context-summary header; got: {first_text}"
        );
        // Second message: assistant ack.
        assert_eq!(result[1].role, "assistant");
    }

    // ── try_mark_permanent_failed_fresh ───────────────────────────────────────

    /// When the promise is `None` in KV (already deleted), the function returns
    /// immediately without retrying — no exponential back-off, no error.
    #[tokio::test(start_paused = true)]
    async fn try_mark_permanent_failed_promise_none_returns_immediately() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::MockPromiseStore;

        // Store is empty — get_promise returns Ok(None).
        let store = Arc::new(MockPromiseStore::new());
        let store_as_repo: Arc<dyn PromiseRepository> = store;

        // If the function retried, the test would advance through the back-off
        // sleeps and complete very slowly. With start_paused it still completes
        // instantly because there are no sleeps to advance past.
        try_mark_permanent_failed_fresh(store_as_repo.as_ref(), "acme", "missing-pid", "test")
            .await;
        // Reaching here means the function returned — success.
    }

    /// When every `get_promise` call returns an `Err`, the function retries up
    /// to 5 times (with exponential back-off) and then gives up — no panic,
    /// no infinite loop.
    #[tokio::test(start_paused = true)]
    async fn try_mark_permanent_failed_all_get_errors_exhausts_retries() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::ErrorGetPromiseStore;

        let store = Arc::new(ErrorGetPromiseStore::new());
        let store_as_repo: Arc<dyn PromiseRepository> =
            Arc::clone(&store) as Arc<dyn PromiseRepository>;

        // Back-off sleeps: attempt 1→2s, 2→4s, 3→8s, 4→16s  (attempt 0 has no sleep).
        // With start_paused these advance instantly — the whole test completes synchronously.
        try_mark_permanent_failed_fresh(store_as_repo.as_ref(), "acme", "pid-err", "test ctx")
            .await;
        // Function returns after exhausting retries — no panic.
    }

    /// When `get_promise` hangs indefinitely (simulating a KV timeout), the
    /// NATS_KV_TIMEOUT inside `try_mark_permanent_failed_fresh` fires and the
    /// function retries. After 5 timeouts it gives up without panicking.
    #[tokio::test(start_paused = true)]
    async fn try_mark_permanent_failed_all_get_timeouts_exhausts_retries() {
        use crate::promise_store::PromiseRepository;
        use crate::promise_store::mock::HangingGetPromiseStore;

        let store = Arc::new(HangingGetPromiseStore::new());
        let store_as_repo: Arc<dyn PromiseRepository> =
            Arc::clone(&store) as Arc<dyn PromiseRepository>;

        // Each attempt hangs → NATS_KV_TIMEOUT (10s) fires → retry sleep → next attempt.
        // Tokio's paused clock auto-advances all sleeps instantly.
        try_mark_permanent_failed_fresh(store_as_repo.as_ref(), "acme", "pid-hang", "test ctx")
            .await;
        // Returns after all 5 attempts time out — no panic.
    }

    /// Happy path: promise is found and `write_promise_terminal` marks it
    /// `PermanentFailed` on the first attempt.
    #[tokio::test(start_paused = true)]
    async fn try_mark_permanent_failed_found_promise_marks_it() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("pid-found"));
        let store_as_repo: Arc<dyn PromiseRepository> =
            Arc::clone(&store) as Arc<dyn PromiseRepository>;

        try_mark_permanent_failed_fresh(store_as_repo.as_ref(), "acme", "pid-found", "test ctx")
            .await;

        let (p, _) = store
            .get_promise("acme", "pid-found")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "promise must be PermanentFailed after try_mark_permanent_failed_fresh"
        );
    }

    /// First `get_promise` call returns `Err` (transient KV error) — the function
    /// sleeps the 2-second backoff and retries.  On the second attempt it finds
    /// the promise and marks it `PermanentFailed`.
    ///
    /// Uses `start_paused = true` so the 2-second back-off resolves instantly.
    #[tokio::test(start_paused = true)]
    async fn try_mark_permanent_failed_first_error_second_succeeds_marks_permanent_failed() {
        use crate::promise_store::{
            AgentPromise, PromiseEntry, PromiseRepository, PromiseStatus, PromiseStoreError,
        };
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Store wrapper: get_promise call 0 → Err; call 1+ → delegate.
        struct FirstErrThenOkStore {
            inner: crate::promise_store::mock::MockPromiseStore,
            get_count: Arc<AtomicUsize>,
        }

        impl PromiseRepository for FirstErrThenOkStore {
            fn get_promise<'a>(
                &'a self,
                tenant_id: &'a str,
                promise_id: &'a str,
            ) -> Pin<
                Box<
                    dyn Future<Output = Result<Option<PromiseEntry>, PromiseStoreError>>
                        + Send
                        + 'a,
                >,
            > {
                let n = self.get_count.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    return Box::pin(async {
                        Err(PromiseStoreError(
                            "transient KV error on first attempt".to_string(),
                        ))
                    });
                }
                self.inner.get_promise(tenant_id, promise_id)
            }

            fn put_promise<'a>(
                &'a self,
                promise: &'a AgentPromise,
            ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>>
            {
                self.inner.put_promise(promise)
            }

            fn update_promise<'a>(
                &'a self,
                tenant_id: &'a str,
                promise_id: &'a str,
                promise: &'a AgentPromise,
                revision: u64,
            ) -> Pin<Box<dyn Future<Output = Result<u64, PromiseStoreError>> + Send + 'a>>
            {
                self.inner
                    .update_promise(tenant_id, promise_id, promise, revision)
            }

            fn get_tool_result<'a>(
                &'a self,
                tenant_id: &'a str,
                promise_id: &'a str,
                cache_key: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<Option<String>, PromiseStoreError>> + Send + 'a>>
            {
                self.inner.get_tool_result(tenant_id, promise_id, cache_key)
            }

            fn put_tool_result<'a>(
                &'a self,
                tenant_id: &'a str,
                promise_id: &'a str,
                cache_key: &'a str,
                result: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<(), PromiseStoreError>> + Send + 'a>>
            {
                self.inner
                    .put_tool_result(tenant_id, promise_id, cache_key, result)
            }

            fn list_running<'a>(
                &'a self,
                tenant_id: &'a str,
            ) -> Pin<
                Box<dyn Future<Output = Result<Vec<AgentPromise>, PromiseStoreError>> + Send + 'a>,
            > {
                self.inner.list_running(tenant_id)
            }
        }

        let store = Arc::new(FirstErrThenOkStore {
            inner: crate::promise_store::mock::MockPromiseStore::new(),
            get_count: Arc::new(AtomicUsize::new(0)),
        });
        store.inner.insert_promise(make_test_promise("pid-retry"));
        let store_as_repo: Arc<dyn PromiseRepository> =
            Arc::clone(&store) as Arc<dyn PromiseRepository>;

        // With start_paused=true the 2-second back-off (attempt 1) resolves instantly.
        try_mark_permanent_failed_fresh(store_as_repo.as_ref(), "acme", "pid-retry", "test ctx")
            .await;

        let (p, _) = store
            .inner
            .get_promise("acme", "pid-retry")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "promise must be PermanentFailed after retry succeeds"
        );
    }

    // ── Pre-pin system prompt ─────────────────────────────────────────────────
    //
    // The pre-pin block (lines 908–1017) writes system_prompt to KV before the
    // first LLM call on fresh (non-recovering) runs. It has a full
    // CAS-conflict path and an initial-timeout path, each with reload + retry.
    //
    // Call sequence for a fresh run WITH a system_prompt:
    //   update_promise [Initial] → pre-pin write        (configurable)
    //   get_promise    [Reload]  → reload after failure (configurable)
    //   update_promise [Retry]   → retry write          (configurable)
    //   update_promise [Done]    → heartbeat write      (delegates to inner)
    //   (LLM call → end_turn)
    //   update_promise [Done]    → terminal Resolved    (delegates to inner)

    #[derive(Clone, PartialEq)]
    enum PrePinPhase {
        Initial,
        Reload,
        Retry,
        Done,
    }

    #[derive(Clone)]
    enum PrePinInitial {
        Succeed,
        CasConflict,
        Hang,
    }

    #[derive(Clone)]
    enum PrePinReload {
        /// get_promise returns the inner promise (system_prompt = None) → Retry.
        Succeed,
        /// get_promise returns the inner promise but with system_prompt set →
        /// simulates the timed-out write having landed before we observed it.
        AlreadyWritten,
        /// get_promise returns Ok(None) → fallback, no retry.
        None_,
        /// get_promise returns Err → fallback, no retry.
        Err_,
        /// get_promise returns pending() → NATS_KV_TIMEOUT fires → fallback.
        Hang,
    }

    #[derive(Clone)]
    enum PrePinRetry {
        Succeed,
        Err_,
        Hang,
    }

    struct PrePinTestStore {
        inner: crate::promise_store::mock::MockPromiseStore,
        phase: Arc<std::sync::Mutex<PrePinPhase>>,
        initial: PrePinInitial,
        reload: PrePinReload,
        retry: PrePinRetry,
    }

    impl PrePinTestStore {
        fn new(initial: PrePinInitial, reload: PrePinReload, retry: PrePinRetry) -> Self {
            Self {
                inner: crate::promise_store::mock::MockPromiseStore::new(),
                phase: Arc::new(std::sync::Mutex::new(PrePinPhase::Initial)),
                initial,
                reload,
                retry,
            }
        }
    }

    impl crate::promise_store::PromiseRepository for PrePinTestStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<crate::promise_store::PromiseEntry>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            let mut phase = self.phase.lock().unwrap();
            if *phase != PrePinPhase::Reload {
                drop(phase);
                return self.inner.get_promise(tenant_id, promise_id);
            }
            // Reload phase — fire configured behavior and advance to next phase.
            match &self.reload {
                PrePinReload::Succeed => {
                    *phase = PrePinPhase::Retry;
                    drop(phase);
                    // inner has the promise with system_prompt = None — correct for retry path
                    self.inner.get_promise(tenant_id, promise_id)
                }
                PrePinReload::AlreadyWritten => {
                    *phase = PrePinPhase::Done;
                    let inner = self.inner.clone();
                    let t = tenant_id.to_string();
                    let p_id = promise_id.to_string();
                    drop(phase);
                    Box::pin(async move {
                        match inner.get_promise(&t, &p_id).await {
                            Ok(Some((mut entry, rev))) => {
                                // Simulate the write having landed: return the same
                                // revision so subsequent writes don't CAS-conflict.
                                entry.system_prompt = Some("test system prompt".into());
                                Ok(Some((entry, rev)))
                            }
                            other => other,
                        }
                    })
                }
                PrePinReload::None_ => {
                    *phase = PrePinPhase::Done;
                    drop(phase);
                    Box::pin(async { Ok(None) })
                }
                PrePinReload::Err_ => {
                    *phase = PrePinPhase::Done;
                    drop(phase);
                    Box::pin(async {
                        Err(crate::promise_store::PromiseStoreError(
                            "injected pre-pin reload error".into(),
                        ))
                    })
                }
                PrePinReload::Hang => {
                    *phase = PrePinPhase::Done;
                    drop(phase);
                    Box::pin(std::future::pending())
                }
            }
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a crate::promise_store::AgentPromise,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a crate::promise_store::AgentPromise,
            revision: u64,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            let mut phase = self.phase.lock().unwrap();
            match &*phase {
                PrePinPhase::Initial => match &self.initial {
                    PrePinInitial::Succeed => {
                        *phase = PrePinPhase::Done;
                        drop(phase);
                        self.inner
                            .update_promise(tenant_id, promise_id, promise, revision)
                    }
                    PrePinInitial::CasConflict => {
                        *phase = PrePinPhase::Reload;
                        drop(phase);
                        Box::pin(async {
                            Err(crate::promise_store::PromiseStoreError(
                                "injected pre-pin CAS conflict".into(),
                            ))
                        })
                    }
                    PrePinInitial::Hang => {
                        // Set phase before returning pending so the reload
                        // get_promise call sees Reload when the timeout fires.
                        *phase = PrePinPhase::Reload;
                        drop(phase);
                        Box::pin(std::future::pending())
                    }
                },
                PrePinPhase::Retry => match &self.retry {
                    PrePinRetry::Succeed => {
                        *phase = PrePinPhase::Done;
                        let inner = self.inner.clone();
                        let t = tenant_id.to_string();
                        let p_id = promise_id.to_string();
                        let p_clone = promise.clone();
                        drop(phase);
                        Box::pin(async move {
                            inner.update_promise(&t, &p_id, &p_clone, revision).await
                        })
                    }
                    PrePinRetry::Err_ => {
                        *phase = PrePinPhase::Done;
                        drop(phase);
                        Box::pin(async {
                            Err(crate::promise_store::PromiseStoreError(
                                "injected pre-pin retry error".into(),
                            ))
                        })
                    }
                    PrePinRetry::Hang => {
                        *phase = PrePinPhase::Done;
                        drop(phase);
                        Box::pin(std::future::pending())
                    }
                },
                // Done (heartbeat, terminal) and Reload (shouldn't receive writes)
                // — delegate to inner.
                PrePinPhase::Done | PrePinPhase::Reload => {
                    drop(phase);
                    self.inner
                        .update_promise(tenant_id, promise_id, promise, revision)
                }
            }
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<String>, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner
                .put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<crate::promise_store::AgentPromise>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.list_running(tenant_id)
        }
    }

    fn make_pre_pin_agent(
        anthropic: Arc<dyn AnthropicClient>,
        store: Arc<PrePinTestStore>,
    ) -> AgentLoop {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::PromiseRepository;
        use crate::tools::ToolContext;

        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        AgentLoop {
            anthropic_client: anthropic,
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(crate::tools::mock::MockToolDispatcher::new("unused")),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: Some(Arc::clone(&store) as Arc<dyn PromiseRepository>),
            promise_id: Some("p1".to_string()),
            permission_checker: None,
            elicitation_provider: None,
        }
    }

    /// Happy path: pre-pin write succeeds on the first try — system_prompt is
    /// persisted to KV and the run completes normally.
    #[tokio::test]
    async fn pre_pin_success_writes_system_prompt_to_store() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::Succeed,
            PrePinReload::None_,  // not reached
            PrePinRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent
            .run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt"),
            )
            .await;
        assert_eq!(result.unwrap(), "done");

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
        assert_eq!(
            p.system_prompt.as_deref(),
            Some("test system prompt"),
            "system_prompt must be persisted to KV on successful pre-pin write"
        );
    }

    /// CAS conflict on pre-pin write → reload the current revision → retry
    /// write succeeds — system_prompt is persisted and the run completes.
    #[tokio::test]
    async fn pre_pin_cas_conflict_reload_retry_success_writes_system_prompt() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::CasConflict,
            PrePinReload::Succeed,
            PrePinRetry::Succeed,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent
            .run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt"),
            )
            .await;
        assert_eq!(result.unwrap(), "done");

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
        assert_eq!(
            p.system_prompt.as_deref(),
            Some("test system prompt"),
            "system_prompt must be persisted via retry write after CAS conflict"
        );
    }

    /// CAS conflict on pre-pin write → reload → retry write fails — run still
    /// completes; system_prompt stays in-memory for the first checkpoint write.
    #[tokio::test]
    async fn pre_pin_cas_conflict_reload_retry_error_run_completes() {
        use crate::promise_store::PromiseStatus;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::CasConflict,
            PrePinReload::Succeed,
            PrePinRetry::Err_,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent
            .run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt"),
            )
            .await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when pre-pin retry fails"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// CAS conflict on pre-pin write → reload → retry write hangs past
    /// `NATS_KV_TIMEOUT` — run still completes.
    #[tokio::test(start_paused = true)]
    async fn pre_pin_cas_conflict_reload_retry_timeout_run_completes() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::CasConflict,
            PrePinReload::Succeed,
            PrePinRetry::Hang,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt")
            ),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when pre-pin retry times out"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// CAS conflict on pre-pin write → reload returns `None` (promise vanished)
    /// — run still completes (system_prompt kept in-memory only).
    #[tokio::test]
    async fn pre_pin_cas_conflict_reload_none_run_completes() {
        use crate::promise_store::PromiseStatus;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::CasConflict,
            PrePinReload::None_,
            PrePinRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent
            .run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt"),
            )
            .await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete when reload returns None"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// CAS conflict on pre-pin write → reload returns `Err` — run still
    /// completes; system_prompt kept in-memory only.
    #[tokio::test]
    async fn pre_pin_cas_conflict_reload_error_run_completes() {
        use crate::promise_store::PromiseStatus;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::CasConflict,
            PrePinReload::Err_,
            PrePinRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let result = agent
            .run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt"),
            )
            .await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete when reload errors"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// CAS conflict on pre-pin write → reload hangs past `NATS_KV_TIMEOUT` —
    /// run still completes.
    #[tokio::test(start_paused = true)]
    async fn pre_pin_cas_conflict_reload_timeout_run_completes() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::CasConflict,
            PrePinReload::Hang,
            PrePinRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt")
            ),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete when pre-pin reload times out"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// Initial pre-pin write times out → reload shows the write already landed
    /// (system_prompt is set) — no retry needed, run completes.
    #[tokio::test(start_paused = true)]
    async fn pre_pin_initial_timeout_reload_already_written_run_completes() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::Hang,
            PrePinReload::AlreadyWritten,
            PrePinRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt")
            ),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete when write landed before timeout was observed"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// Initial pre-pin write times out → reload shows write did not land →
    /// retry write succeeds — system_prompt is persisted and run completes.
    #[tokio::test(start_paused = true)]
    async fn pre_pin_initial_timeout_reload_retry_success_writes_system_prompt() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use std::time::Duration;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::Hang,
            PrePinReload::Succeed,
            PrePinRetry::Succeed,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt")
            ),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(result.unwrap(), "done");

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
        assert_eq!(
            p.system_prompt.as_deref(),
            Some("test system prompt"),
            "system_prompt must be persisted via retry after initial write timeout"
        );
    }

    /// Initial pre-pin write times out → reload shows write did not land →
    /// retry write fails — run still completes; system_prompt kept in-memory.
    #[tokio::test(start_paused = true)]
    async fn pre_pin_initial_timeout_reload_retry_error_run_completes() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::Hang,
            PrePinReload::Succeed,
            PrePinRetry::Err_,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt")
            ),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete when retry write errors after initial timeout"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// Initial pre-pin write times out → reload shows write did not land →
    /// retry write also times out — run still completes.
    #[tokio::test(start_paused = true)]
    async fn pre_pin_initial_timeout_reload_retry_timeout_run_completes() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::Hang,
            PrePinReload::Succeed,
            PrePinRetry::Hang,
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        // Two NATS_KV_TIMEOUT waits fire: initial write + retry write.
        let (result, _) = tokio::join!(
            agent.run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt")
            ),
            tokio::time::advance(NATS_KV_TIMEOUT * 2 + Duration::from_millis(2)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete when both pre-pin writes time out"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    /// Initial pre-pin write times out → reload fails — run still completes;
    /// system_prompt kept in-memory for the first checkpoint write.
    #[tokio::test(start_paused = true)]
    async fn pre_pin_initial_timeout_reload_error_run_completes() {
        use crate::promise_store::PromiseStatus;
        use std::time::Duration;

        let store = Arc::new(PrePinTestStore::new(
            PrePinInitial::Hang,
            PrePinReload::Err_,
            PrePinRetry::Succeed, // not reached
        ));
        store.inner.insert_promise(make_test_promise("p1"));

        let agent = make_pre_pin_agent(end_turn_anthropic(), Arc::clone(&store));
        let (result, _) = tokio::join!(
            agent.run(
                vec![Message::user_text("go")],
                &[],
                Some("test system prompt")
            ),
            tokio::time::advance(NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete when reload errors after initial timeout"
        );

        let (p, _) = store
            .inner
            .get_promise("acme", "p1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.status, PromiseStatus::Resolved);
    }

    // ── Checkpoint trim: summarize timeout ────────────────────────────────────
    //
    // When the checkpoint payload exceeds CHECKPOINT_MAX_BYTES, the agent trims
    // old messages and calls the LLM to summarize the dropped messages.  That
    // call is wrapped in a 30-second timeout; on timeout `unwrap_or_default()`
    // yields an empty Vec and the code falls back to a plain trim.

    /// Summarize LLM call hangs past the 30-second cap → `unwrap_or_default()`
    /// triggers plain-trim fallback → checkpoint is written and run completes.
    #[tokio::test(start_paused = true)]
    async fn checkpoint_trim_summarize_timeout_falls_back_to_plain_trim() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Anthropic client: call 0 → tool_use, call 1 → tool_use,
        // call 2 → pending() (summarize), call 3+ → end_turn.
        struct SummarizeHangClient {
            count: Arc<AtomicUsize>,
        }
        impl AnthropicClient for SummarizeHangClient {
            fn complete<'a>(
                &'a self,
                _body: serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>>
            {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                let tool_use_resp = serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
                });
                let end_turn_resp = serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "done"}]
                });
                match n {
                    0 | 1 => Box::pin(async move { Ok(tool_use_resp) }),
                    2 => Box::pin(std::future::pending()),
                    _ => Box::pin(async move { Ok(end_turn_resp) }),
                }
            }
        }

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let anthropic = Arc::new(SummarizeHangClient {
            count: Arc::new(AtomicUsize::new(0)),
        });
        let agent = make_durable_agent(
            anthropic,
            Arc::new(MockToolDispatcher::new("ok")),
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        // Initial message large enough that after two tool turns the checkpoint
        // (5 messages including this) exceeds CHECKPOINT_MAX_BYTES and triggers
        // trimming on turn 2.  Turn 1 has only 3 messages (< MIN_TRIM_KEEP=4)
        // so checkpointing is disabled there; turn 2 gets 5 messages and trim
        // succeeds by dropping the large initial message.
        let large_text = "a".repeat(CHECKPOINT_MAX_BYTES + 1);

        // The summarize call (LLM call 2) hangs for 30 s before timing out.
        // tokio's paused clock auto-advances through the 30-second timeout and
        // any subsequent NATS_KV_TIMEOUT waits without manual clock control.
        let result = agent
            .run(vec![Message::user_text(large_text)], &[], None)
            .await;
        assert_eq!(
            result.unwrap(),
            "done",
            "run must complete even when summarize times out"
        );

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after successful run"
        );
        assert!(
            !p.messages.is_empty(),
            "checkpoint must be written with trimmed messages after plain-trim fallback"
        );
    }

    /// When `input_tokens` in a tool_use response exceeds `CONTEXT_TOKEN_THRESHOLD`,
    /// `run()` must trim the live `messages` vec before the next Anthropic call.
    ///
    /// Proof-by-substitution: the mock queue is ordered
    ///   [tool_use(low), tool_use(low), tool_use(HIGH→triggers_trim), summary, end_turn].
    /// If the trim fires, call 4 is consumed by `summarize_dropped_messages` and call 5
    /// is consumed by the main loop, returning "done".
    /// If the trim does not fire, call 4 (the summary response) is consumed by the
    /// main loop as an `end_turn`, returning "Summary of earlier context." — the assert
    /// would fail.
    #[tokio::test]
    async fn live_context_trim_fires_when_input_tokens_exceed_threshold() {
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let low = |id: &'static str| {
            serde_json::json!({
                "stop_reason": "tool_use",
                "content": [{"type": "tool_use", "id": id, "name": "my_tool", "input": {}}],
                "usage": {"input_tokens": 1_000u32, "output_tokens": 50u32}
            })
        };
        let high_usage_tool_use = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu3", "name": "my_tool", "input": {}}],
            // input_tokens > CONTEXT_TOKEN_THRESHOLD (150_000) → trim fires
            "usage": {"input_tokens": 160_000u32, "output_tokens": 50u32}
        });
        let summary_response = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "Summary of earlier context."}]
        });
        let end_turn = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });

        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            low("tu1"),
            low("tu2"),
            high_usage_tool_use,
            summary_response,
            end_turn,
        ]));

        let tool_ctx =
            Arc::new(crate::tools::ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::clone(&anthropic) as Arc<dyn AnthropicClient>,
            model: "test".to_string(),
            max_iterations: 10,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("ok")),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(crate::flag_client::AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("start task")], &[], None)
            .await;

        assert_eq!(
            result.unwrap(),
            "done",
            "trim must consume the summary response (call 4) and return 'done' (call 5)"
        );
    }

    /// When `summarize_dropped_messages` fails (LLM returns a malformed response),
    /// the live context trim must fall back to a plain slice of the tail and the
    /// run must still complete successfully.
    #[tokio::test]
    async fn live_context_trim_falls_back_to_plain_trim_on_summarize_failure() {
        use crate::tools::mock::MockToolDispatcher;
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CallCountClient {
            count: Arc<AtomicUsize>,
        }

        impl AnthropicClient for CallCountClient {
            fn complete<'a>(
                &'a self,
                _body: serde_json::Value,
            ) -> Pin<
                Box<dyn Future<Output = Result<serde_json::Value, AnthropicClientError>> + Send + 'a>,
            > {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                let low = serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{"type": "tool_use", "id": "tu", "name": "my_tool", "input": {}}],
                    "usage": {"input_tokens": 1_000u32, "output_tokens": 50u32}
                });
                let high = serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{"type": "tool_use", "id": "tu", "name": "my_tool", "input": {}}],
                    "usage": {"input_tokens": 160_000u32, "output_tokens": 50u32}
                });
                // call 3 (n=3): summarize_dropped_messages — return malformed JSON
                // so deserialization fails and the function returns vec![].
                let malformed = serde_json::json!({});
                let end = serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "done"}]
                });
                let resp = match n {
                    0 | 1 => low,
                    2 => high,
                    3 => malformed,
                    _ => end,
                };
                Box::pin(async move { Ok(resp) })
            }
        }

        let count = Arc::new(AtomicUsize::new(0));
        let tool_ctx =
            Arc::new(crate::tools::ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        let agent = AgentLoop {
            anthropic_client: Arc::new(CallCountClient {
                count: Arc::clone(&count),
            }),
            model: "test".to_string(),
            max_iterations: 10,
            tool_dispatcher: Arc::new(MockToolDispatcher::new("ok")),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(crate::flag_client::AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: None,
        };

        let result = agent
            .run(vec![Message::user_text("start task")], &[], None)
            .await;

        assert_eq!(
            result.unwrap(),
            "done",
            "plain trim fallback must allow run to complete after summarize failure"
        );
        assert_eq!(
            count.load(Ordering::SeqCst),
            5,
            "must make exactly 5 calls: tu1, tu2, tu3(high), summarize(fails), end_turn"
        );
    }

    // ── ElicitationProvider / ask_user ────────────────────────────────────────

    struct ConstElicitation(Option<String>);

    impl ElicitationProvider for ConstElicitation {
        fn elicit<'a>(
            &'a self,
            _question: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
            let val = self.0.clone();
            Box::pin(async move { val })
        }
    }

    fn make_ask_user_agent(elicitation: Option<Arc<dyn ElicitationProvider>>) -> AgentLoop {
        use crate::agent_loop::mock::SequencedMockAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};

        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        AgentLoop {
            anthropic_client: Arc::new(SequencedMockAnthropicClient::new(vec![
                // First call: LLM requests ask_user tool.
                serde_json::json!({
                    "stop_reason": "tool_use",
                    "content": [{
                        "type": "tool_use",
                        "id": "tu-ask",
                        "name": "ask_user",
                        "input": { "question": "What is your name?" }
                    }],
                    "usage": { "input_tokens": 10, "output_tokens": 5 }
                }),
                // Second call: LLM finishes.
                serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{ "type": "text", "text": "Got it." }],
                    "usage": { "input_tokens": 15, "output_tokens": 4 }
                }),
            ])),
            model: "test".to_string(),
            max_iterations: 5,
            tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test-tenant".to_string(),
            promise_store: None,
            promise_id: None,
            permission_checker: None,
            elicitation_provider: elicitation,
        }
    }

    /// When `elicitation_provider` returns `Some(answer)`, `run_chat` feeds that
    /// answer back to the LLM as the tool result and completes successfully.
    #[tokio::test]
    async fn run_chat_ask_user_with_provider_returning_answer_forwards_answer_to_llm() {
        let provider = Arc::new(ConstElicitation(Some("Alice".to_string()))) as Arc<dyn ElicitationProvider>;
        let agent = make_ask_user_agent(Some(provider));

        let (_, updated) = agent
            .run_chat(vec![Message::user_text("ask me something")], &[], None)
            .await
            .expect("run_chat must succeed");

        // The tool_result message in the history must contain the elicitation answer.
        let has_answer = updated.iter().any(|m| {
            m.content.iter().any(|b| {
                matches!(b, ContentBlock::ToolResult { content, .. } if content.contains("Alice"))
            })
        });
        assert!(
            has_answer,
            "tool_result message must contain the elicitation answer 'Alice', got: {updated:?}"
        );
    }

    /// When `elicitation_provider` returns `None` (user declined/cancelled),
    /// the "declined" message is sent back to the LLM and the run completes.
    #[tokio::test]
    async fn run_chat_ask_user_provider_returning_none_sends_declined_message_to_llm() {
        let provider = Arc::new(ConstElicitation(None)) as Arc<dyn ElicitationProvider>;
        let agent = make_ask_user_agent(Some(provider));

        let (_, updated) = agent
            .run_chat(vec![Message::user_text("ask me something")], &[], None)
            .await
            .expect("run_chat must succeed");

        let tool_result_msg = updated.iter().find(|m| {
            m.content.iter().any(|b| {
                matches!(b, ContentBlock::ToolResult { content, .. }
                    if content.contains("declined") || content.contains("cancelled"))
            })
        });
        assert!(
            tool_result_msg.is_some(),
            "tool_result must contain 'declined' or 'cancelled' when provider returns None"
        );
    }

    /// When `elicitation_provider` is `None`, the "not available" message is
    /// sent back to the LLM for any `ask_user` tool call.
    #[tokio::test]
    async fn run_chat_ask_user_without_provider_sends_not_available_to_llm() {
        let agent = make_ask_user_agent(None); // no elicitation_provider

        let (_, updated) = agent
            .run_chat(vec![Message::user_text("ask me something")], &[], None)
            .await
            .expect("run_chat must succeed");

        let tool_result_msg = updated.iter().find(|m| {
            m.content.iter().any(|b| {
                matches!(b, ContentBlock::ToolResult { content, .. }
                    if content.contains("not available"))
            })
        });
        assert!(
            tool_result_msg.is_some(),
            "tool_result must say 'not available' when elicitation_provider is None"
        );
    }

    // ── reqwest_error_to_anthropic_error ──────────────────────────────────────

    /// A 4xx response (that is not 429) must produce `AnthropicClientError::Permanent`.
    #[tokio::test]
    async fn reqwest_error_4xx_not_429_maps_to_permanent() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/test");
            then.status(400);
        });

        let resp = reqwest::get(format!("{}/test", server.base_url()))
            .await
            .unwrap();
        let err = resp.error_for_status().unwrap_err();
        let result = super::reqwest_error_to_anthropic_error(err);
        assert!(
            matches!(result, AnthropicClientError::Permanent(_)),
            "400 must map to Permanent, got: {result:?}"
        );
    }

    /// A 429 response must produce `AnthropicClientError::Transient` (it is
    /// rate-limiting, which is retryable — unlike other 4xx errors).
    #[tokio::test]
    async fn reqwest_error_429_maps_to_transient() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/test");
            then.status(429);
        });

        let resp = reqwest::get(format!("{}/test", server.base_url()))
            .await
            .unwrap();
        let err = resp.error_for_status().unwrap_err();
        let result = super::reqwest_error_to_anthropic_error(err);
        assert!(
            matches!(result, AnthropicClientError::Transient(_)),
            "429 must map to Transient, got: {result:?}"
        );
    }

    /// A 5xx response must produce `AnthropicClientError::Transient`.
    #[tokio::test]
    async fn reqwest_error_5xx_maps_to_transient() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/test");
            then.status(503);
        });

        let resp = reqwest::get(format!("{}/test", server.base_url()))
            .await
            .unwrap();
        let err = resp.error_for_status().unwrap_err();
        let result = super::reqwest_error_to_anthropic_error(err);
        assert!(
            matches!(result, AnthropicClientError::Transient(_)),
            "503 must map to Transient, got: {result:?}"
        );
    }
}
