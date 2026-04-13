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

// ── AnthropicClient trait ──────────────────────────────────────────────────────

/// Trait for sending a request to the Anthropic messages API.
pub trait AnthropicClient: Send + Sync + 'static {
    fn complete<'a>(
        &'a self,
        body: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>>;
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
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>> {
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
            // for those status codes, so they propagate immediately to the caller.
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
                    Err(e) => return Err(e),
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
                    Err(e) => return Err(e),
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
                    Err(e) => return Err(e),
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
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>>
        {
            let resp = self.response.clone();
            Box::pin(async move { Ok(resp) })
        }
    }

    /// Mock that returns responses from a pre-loaded queue, in order.
    ///
    /// Panics if called when the queue is empty — useful for asserting Anthropic
    /// is never called (pass an empty `Vec`).
    pub struct SequencedMockAnthropicClient {
        responses: Mutex<VecDeque<serde_json::Value>>,
    }

    impl SequencedMockAnthropicClient {
        pub fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
            }
        }
    }

    impl AnthropicClient for SequencedMockAnthropicClient {
        fn complete<'a>(
            &'a self,
            _body: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>>
        {
            let resp = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("SequencedMockAnthropicClient ran out of queued responses");
            Box::pin(async move { Ok(resp) })
        }
    }
}

// ── Wire types ────────────────────────────────────────────────────────────────

/// A single message in the Anthropic conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// Simple user turn with plain text.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// Assistant turn (used when appending a model response to history).
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
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
    messages: &'a [Message],
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    stop_reason: String,
    content: Vec<ContentBlock>,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AgentError {
    Http(reqwest::Error),
    MaxIterationsReached,
    UnexpectedStopReason(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::MaxIterationsReached => write!(f, "Agent exceeded max iterations"),
            Self::UnexpectedStopReason(r) => write!(f, "Unexpected stop reason: {r}"),
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
pub(crate) const TOOL_EXECUTION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(60);

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
    match tokio::time::timeout(NATS_KV_TIMEOUT, store.update_promise(tenant_id, pid, promise, rev)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = %e, context, "Failed to write terminal promise status"),
        Err(_) => warn!(promise_id = %pid, context, "NATS KV write timed out writing terminal promise status"),
    }
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
        let mut checkpointing_disabled = false;
        // `recovering` is true only when we loaded a checkpoint — used to gate
        // the tool-result cache replay in `execute_tools`.
        let mut recovering = false;
        if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
            let initial_load = match tokio::time::timeout(
                NATS_KV_TIMEOUT,
                store.get_promise(&self.tenant_id, pid),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => {
                    warn!(promise_id = %pid, "NATS KV get_promise timed out on initial load — starting fresh without checkpoint");
                    Ok(None)
                }
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

        // Mark the last tool with cache_control so Anthropic caches the entire
        // tool definitions block across repeated requests.
        let mut cached_tools: Vec<ToolDef> = all_tools;
        if let Some(last) = cached_tools.last_mut() {
            last.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
        }

        for iteration in 0..self.max_iterations {
            debug!(iteration, "Agent loop iteration");

            // Build the cacheable system block on each iteration (cheap — just wraps a &str).
            let system: Option<Vec<SystemBlock<'_>>> = effective_prompt.as_deref().map(|text| {
                vec![SystemBlock {
                    block_type: "text",
                    text,
                    cache_control: CacheControl::ephemeral(),
                }]
            });

            let request = AnthropicRequest {
                model: &self.model,
                max_tokens: 4096,
                system,
                tools: &cached_tools,
                messages: &messages,
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
                        let terminal_status = if e
                            .status()
                            .map(|s| s.is_client_error() && s != 429)
                            .unwrap_or(false)
                        {
                            crate::promise_store::PromiseStatus::PermanentFailed
                        } else {
                            crate::promise_store::PromiseStatus::Failed
                        };
                        write_promise_terminal(store.as_ref(), &self.tenant_id, pid, p, *rev, terminal_status, "HTTP error").await;
                    }
                    return Err(AgentError::Http(e));
                }
            };
            let response = match serde_json::from_value::<AnthropicResponse>(raw) {
                Ok(r) => r,
                Err(e) => {
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                        && let Some((ref mut p, ref mut rev)) = checkpoint
                    {
                        // Deterministic: retrying the same response will always fail.
                        write_promise_terminal(store.as_ref(), &self.tenant_id, pid, p, *rev, crate::promise_store::PromiseStatus::PermanentFailed, "deserialization error").await;
                    }
                    return Err(AgentError::UnexpectedStopReason(format!("Deserialization error: {e}")));
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
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                        && let Some((ref mut p, ref mut rev)) = checkpoint
                    {
                        p.status = crate::promise_store::PromiseStatus::Resolved;
                        match tokio::time::timeout(
                            NATS_KV_TIMEOUT,
                            store.update_promise(&self.tenant_id, pid, p, *rev),
                        )
                        .await
                        {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => warn!(error = %e, "Failed to mark promise Resolved"),
                            Err(_) => warn!(promise_id = %pid, "NATS KV write timed out marking promise Resolved"),
                        }
                    }
                    return Ok(text);
                }
                "tool_use" => {
                    let results = self.execute_tools(&response.content, recovering).await;
                    messages.push(Message::assistant(response.content));
                    messages.push(Message::tool_results(results));
                    // Once we've executed a tool turn, we've caught up with the
                    // checkpoint and are back in normal forward execution.
                    recovering = false;

                    // ── Durable promise: checkpoint after tool turn ───────────
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                        && let Some((ref mut p, ref mut rev)) = checkpoint
                    {
                        if checkpointing_disabled {
                            // Payload was too large on a previous turn — skip the
                            // message-history write but leave `checkpoint` intact so
                            // terminal-status writes (Resolved, PermanentFailed) can
                            // still use the current KV revision.
                        } else {

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
                            p.messages = prev_messages; // restore — p must stay clean
                            warn!(
                                promise_id = %pid,
                                size_bytes = serialized_len,
                                limit_bytes = CHECKPOINT_MAX_BYTES,
                                "Checkpoint payload exceeds NATS KV size limit — disabling checkpointing for this run"
                            );
                            checkpointing_disabled = true;
                        } else {
                        // Size is within bounds — commit all fields to p.
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
                            Ok(Ok(new_rev)) => *rev = new_rev,
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
                                .unwrap_or_else(|_| {
                                    warn!(promise_id = %pid, "NATS KV get_promise timed out during CAS reload — checkpointing disabled for this run");
                                    Ok(None)
                                }) {
                                    Ok(Some((current, new_rev))) => {
                                        // If another worker reached a terminal state
                                        // between our tool turn and the CAS write,
                                        // stop immediately — continuing would produce
                                        // duplicate side effects (extra PR comments,
                                        // Slack messages, etc.) for a run that already
                                        // completed. Return an empty string so the
                                        // caller's ack path still runs cleanly.
                                        if current.status != crate::promise_store::PromiseStatus::Running {
                                            warn!(
                                                promise_id = %pid,
                                                status = ?current.status,
                                                "Promise was resolved/failed by another worker during CAS reload — stopping this run to avoid duplicate outputs"
                                            );
                                            return Ok(String::new());
                                        }
                                        *rev = new_rev;
                                    }
                                    Ok(None) => {
                                        error!(
                                            promise_id = %pid,
                                            "Promise disappeared during CAS reload — checkpointing disabled for this run"
                                        );
                                        // Disable further checkpoint attempts: the
                                        // promise is gone so every subsequent write
                                        // would fail too, and the misleading
                                        // "CAS conflict" log would repeat each turn.
                                        // Side-effect: terminal-status writes (Failed,
                                        // PermanentFailed) are also skipped for the
                                        // rest of this run, since they require a valid
                                        // checkpoint revision. This is acceptable —
                                        // the promise is already gone from KV.
                                        checkpoint = None;
                                    }
                                    Err(e) => {
                                        error!(
                                            error = %e,
                                            promise_id = %pid,
                                            "CAS reload failed — checkpointing disabled for this run; crash recovery will replay from last successful checkpoint"
                                        );
                                        // Same: stop attempting checkpoints so
                                        // future turns don't log spurious
                                        // "CAS conflict" errors for a stale revision.
                                        // Terminal-status writes are also skipped for
                                        // the remainder of this run. If the run hits
                                        // a deterministic error (e.g. max_tokens), the
                                        // promise stays Running until the next startup
                                        // recovery, which reloads the last valid
                                        // checkpoint and can mark it PermanentFailed.
                                        checkpoint = None;
                                    }
                                }
                            }
                            Err(_) => {
                                warn!(
                                    promise_id = %pid,
                                    "NATS KV update_promise timed out — checkpointing disabled for this run"
                                );
                                // Terminal-status writes are also skipped for the
                                // remainder of this run. If the run hits a
                                // deterministic error (e.g. max_tokens), the promise
                                // stays Running until the next startup recovery, which
                                // reloads the last valid checkpoint and can mark it
                                // PermanentFailed on a successful KV write.
                                checkpoint = None;
                            }
                        }
                        } // else: serialized_len <= CHECKPOINT_MAX_BYTES
                        } // else: !checkpointing_disabled
                    }
                }
                other => {
                    let reason = other.to_string();
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                        && let Some((ref mut p, ref mut rev)) = checkpoint
                    {
                        // Deterministic: same stop_reason on every retry.
                        write_promise_terminal(store.as_ref(), &self.tenant_id, pid, p, *rev, crate::promise_store::PromiseStatus::PermanentFailed, "unexpected stop reason").await;
                    }
                    return Err(AgentError::UnexpectedStopReason(reason));
                }
            }
        }

        warn!(max = self.max_iterations, "Agent reached max iterations");
        if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
            && let Some((ref mut p, ref mut rev)) = checkpoint
        {
            // Deterministic: same messages + same max_iterations = same outcome.
            write_promise_terminal(store.as_ref(), &self.tenant_id, pid, p, *rev, crate::promise_store::PromiseStatus::PermanentFailed, "max iterations").await;
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

            let request = AnthropicRequest {
                model: &self.model,
                max_tokens: 4096,
                system,
                tools: &cached_tools,
                messages: &messages,
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

                    messages.push(Message::assistant(response.content));
                    info!(iterations = iteration + 1, "Chat completed");
                    return Ok((text, messages));
                }
                "tool_use" => {
                    let results = self.execute_tools(&response.content, false).await;
                    messages.push(Message::assistant(response.content));
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
    /// `recovering` is `true` when the caller is replaying a turn from a
    /// checkpointed message history after a process restart. In that state,
    /// tool results that were already persisted to KV are replayed without
    /// re-executing the side effect. Once we are back in normal forward
    /// execution (`recovering = false`), the KV cache is never consulted —
    /// Anthropic generates unique tool_use IDs per request, so there is no
    /// risk of replaying a fresh call as a cached one.
    async fn execute_tools(&self, content: &[ContentBlock], recovering: bool) -> Vec<ToolResult> {
        let mut results = Vec::new();

        for block in content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, "Executing tool");

                // ── Durable promise: replay cached result (recovery only) ────
                // Only consult the KV cache when we are recovering from a
                // checkpoint. The cache key is derived from (tool_name, input)
                // rather than tool_use_id, because Anthropic generates a fresh
                // tool_use_id on every request — meaning the old ID would never
                // match after a restart. The content-based key survives the
                // restart: the LLM re-generates the same call with the same
                // name and input, so the hash is identical and the cached result
                // is replayed without re-executing the tool.
                if recovering
                    && let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                {
                    let ck = tool_cache_key(name, input);
                    let cache_result = match tokio::time::timeout(
                        NATS_KV_TIMEOUT,
                        store.get_tool_result(&self.tenant_id, pid, &ck),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            warn!(
                                tool = %name,
                                promise_id = %pid,
                                "NATS KV get_tool_result timed out — re-executing tool"
                            );
                            Ok(None) // treat timeout as cache miss: re-execute
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
                        Err(e) => warn!(error = %e, "Failed to read tool result cache"),
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
                            serde_json::Value::String(format!("{pid}.{}", tool_cache_key(name, input))),
                        );
                    }
                    v
                } else {
                    input.clone()
                };

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
                // Both paths are wrapped with `TOOL_EXECUTION_TIMEOUT` so a
                // hung external API cannot block the run indefinitely while the
                // heartbeat keeps the NATS message alive.
                let output = if let Some((_, original, client)) = self
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
                        Ok(Ok(out)) => out,
                        Ok(Err(e)) => format!("Tool error: {e}"),
                        Err(_) => {
                            warn!(
                                tool = %name,
                                timeout_secs = TOOL_EXECUTION_TIMEOUT.as_secs(),
                                "Tool execution timed out"
                            );
                            format!(
                                "Tool error: execution timed out after {}s",
                                TOOL_EXECUTION_TIMEOUT.as_secs()
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
                        Ok(output) => output,
                        Err(_) => {
                            warn!(
                                tool = %name,
                                timeout_secs = TOOL_EXECUTION_TIMEOUT.as_secs(),
                                "Tool execution timed out"
                            );
                            format!(
                                "Tool error: execution timed out after {}s",
                                TOOL_EXECUTION_TIMEOUT.as_secs()
                            )
                        }
                    }
                };

                // ── Durable promise: persist tool result ─────────────────────
                // Store the result keyed by (tool_name, input) so that on crash
                // recovery — when Anthropic re-generates a fresh tool_use_id for
                // the same call — the cache still matches and the tool is not
                // re-executed.
                if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id) {
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
                                "NATS KV put_tool_result timed out — tool will be re-executed on recovery if process crashes"
                            );
                        }
                    }
                }

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
        // Construct a dummy reqwest error via a failed parse (no network needed).
        let err = reqwest::Client::new()
            .get("not a url at all:///")
            .build()
            .unwrap_err();
        let agent_err = AgentError::Http(err);
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
            messages: &[],
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
            messages: &[],
        };
        let body = serde_json::to_value(&req).unwrap();
        assert!(
            body.get("system").is_none(),
            "system key should be absent when None"
        );
    }

    // ── is_flag_enabled ───────────────────────────────────────────────────────

    fn make_test_agent(split_client: Option<trogon_splitio::SplitClient>) -> AgentLoop {
        use crate::flag_client::{AlwaysOnFlagClient, SplitFlagClient};
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        use std::sync::Arc;

        let http = reqwest::Client::new();
        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));

        let flag_client: Arc<dyn crate::flag_client::FeatureFlagClient> = match split_client {
            Some(c) => Arc::new(SplitFlagClient::new(c)),
            None => Arc::new(AlwaysOnFlagClient),
        };

        AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                http,
                "http://127.0.0.1:1".to_string(),
                String::new(),
            )),
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseStatus;
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
        assert_eq!(result.unwrap(), "", "Resolved promise must return empty string immediately");
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await;
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseStatus;
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

        let result = agent.run(vec![Message::user_text("do stuff")], &[], None).await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(), "",
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
        assert_eq!(result.unwrap(), "recovered", "Failed promise must complete normally on retry");

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
        use crate::tools::mock::MockToolDispatcher;
        use crate::tools::ToolContext;
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
        let anthropic =
            Arc::new(SequencedMockAnthropicClient::new(vec![tool_use_resp(), tool_use_resp()]));
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
    async fn unexpected_stop_reason_marks_promise_permanent_failed() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        // Anthropic returns a structurally valid response with an unrecognised
        // stop_reason. `"max_tokens"` is the realistic trigger for this path.
        let max_tokens_resp = serde_json::json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "truncated"}]
        });
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![max_tokens_resp]));
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

        // Promise must be PermanentFailed — never retried by recovery or NATS redelivery.
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "promise must be PermanentFailed after unexpected stop_reason — deterministic, retry cannot succeed"
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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

        // The error result was cached in KV so crash recovery does not
        // re-execute the hanging tool.
        let ck = super::tool_cache_key("slow_tool", &serde_json::json!({}));
        let cached = store
            .get_tool_result("acme", "p1", &ck)
            .await
            .unwrap()
            .expect("timed-out tool result must be persisted to KV");
        assert!(
            cached.contains("timed out"),
            "cached result must describe the timeout; got: {cached}"
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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
        assert_eq!(result.unwrap(), "done", "run must complete after CAS conflict reload");

        // Promise must be Resolved in the inner store.
        let (p, _) = store.inner.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after run completes despite initial CAS conflict"
        );
    }

    // ── checkpointing_disabled across turns ───────────────────────────────────

    /// When the size guard fires on turn 1, `checkpointing_disabled = true`
    /// persists across all subsequent turns — no message-history writes occur.
    /// The terminal-status write (`Resolved`) still succeeds at end_turn because
    /// `checkpoint` remains `Some` with a valid KV revision.
    #[tokio::test]
    async fn checkpointing_disabled_persists_across_multiple_turns() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use mock::SequencedMockAnthropicClient;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        // Three LLM responses: two tool_use turns and one end_turn.
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
        let anthropic = Arc::new(SequencedMockAnthropicClient::new(vec![
            tool_use_resp(),
            tool_use_resp(),
            end_turn_resp,
        ]));
        let dispatcher = Arc::new(MockToolDispatcher::new("ok"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        // Initial messages large enough to push the checkpoint over CHECKPOINT_MAX_BYTES.
        let large_text = "a".repeat(CHECKPOINT_MAX_BYTES + 1);
        let result = agent
            .run(vec![Message::user_text(large_text)], &[], None)
            .await;

        assert_eq!(result.unwrap(), "done");

        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        // Neither turn wrote message history.
        assert!(p.messages.is_empty(), "no checkpoint must be written for either turn");
        assert_eq!(p.iteration, 0, "iteration must not advance");
        // Terminal write succeeded.
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "Resolved must be written even when checkpointing is disabled"
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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
        store.put_tool_result("acme", "p1", &ck_a, "result_a").await.unwrap();
        store.put_tool_result("acme", "p1", &ck_b, "result_b").await.unwrap();

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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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
                Box<dyn std::future::Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>,
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
        assert_ne!(key_1, key_3, "different tool names must produce different keys");
    }

    // ── is_retryable_error ────────────────────────────────────────────────────

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

    /// Spin up a minimal TCP server that responds with the given HTTP status
    /// code. Returns a real `reqwest::Error` with that status attached so tests
    /// can verify status-based promise state transitions without a real
    /// Anthropic endpoint.
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
        let resp = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap();
        resp.error_for_status().unwrap_err()
    }

    /// A non-retryable 4xx HTTP error (status 400) must write `PermanentFailed`
    /// to the promise store so that neither startup recovery nor NATS
    /// redelivery wastes Anthropic credits on a deterministically broken run.
    #[tokio::test]
    async fn http_4xx_error_marks_promise_permanent_failed() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use crate::tools::mock::MockToolDispatcher;
        use std::sync::Mutex;

        let err = make_http_error(400).await;

        struct ErrorClient(Arc<Mutex<Option<reqwest::Error>>>);
        impl AnthropicClient for ErrorClient {
            fn complete<'a>(
                &'a self,
                _body: serde_json::Value,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<serde_json::Value, reqwest::Error>,
                        > + Send
                        + 'a,
                >,
            > {
                let e = self.0.lock().unwrap().take().expect("error already consumed");
                Box::pin(async move { Err(e) })
            }
        }

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let agent = make_durable_agent(
            Arc::new(ErrorClient(Arc::new(Mutex::new(Some(err))))),
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
        use std::sync::Mutex;

        let err = make_http_error(500).await;

        struct ErrorClient(Arc<Mutex<Option<reqwest::Error>>>);
        impl AnthropicClient for ErrorClient {
            fn complete<'a>(
                &'a self,
                _body: serde_json::Value,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<serde_json::Value, reqwest::Error>,
                        > + Send
                        + 'a,
                >,
            > {
                let e = self.0.lock().unwrap().take().expect("error already consumed");
                Box::pin(async move { Err(e) })
            }
        }

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let agent = make_durable_agent(
            Arc::new(ErrorClient(Arc::new(Mutex::new(Some(err))))),
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
        use crate::promise_store::mock::TerminalOnCasReloadStore;
        use crate::promise_store::PromiseRepository;
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
        use crate::promise_store::mock::DisappearedOnCasReloadStore;
        use crate::promise_store::PromiseRepository;
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
                assert!(s.contains("Deserialization"), "error must mention Deserialization; got: {s}");
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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

        agent.run(vec![Message::user_text("go")], &[], None).await.unwrap();

        let input = captured.lock().unwrap().take().expect("dispatcher was never called");
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
        assert_eq!(hash_part.len(), 64, "hash must be 64 hex chars; got: {hash_part}");
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
        use crate::promise_store::mock::HangingUpdateStore;
        use crate::promise_store::PromiseRepository;
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
        let (p, _) = store.inner.get_promise("acme", "p1").await.unwrap().unwrap();
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::{tool_def, ToolContext, ToolDispatcher};
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
        };

        agent.run(vec![Message::user_text("go")], &[], None).await.unwrap();

        let input = captured.lock().unwrap().take().expect("MCP client was never called");
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

    /// When `get_tool_result` times out during recovery, the timeout is treated
    /// as a cache miss and the tool is re-executed rather than blocking the run.
    ///
    /// This ensures a degraded NATS connection does not permanently stall a
    /// recovering run — worst case is one extra tool execution, not a hang.
    #[tokio::test(start_paused = true)]
    async fn get_tool_result_kv_timeout_treats_as_cache_miss_and_reexecutes() {
        use crate::promise_store::mock::HangingGetToolResultStore;
        use crate::promise_store::PromiseRepository;
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
            "content": [{"type": "text", "text": "done after cache miss"}]
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

        assert_eq!(result.unwrap(), "done after cache miss");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "tool must be re-executed when get_tool_result KV read times out"
        );
    }

    // ── put_tool_result KV timeout ────────────────────────────────────────────

    /// When `put_tool_result` times out, the run continues normally — but the
    /// tool result is not persisted.  A subsequent crash would cause the tool
    /// to be re-executed on recovery rather than replayed from cache.
    #[tokio::test(start_paused = true)]
    async fn put_tool_result_kv_timeout_run_completes_but_result_not_cached() {
        use crate::promise_store::mock::HangingPutToolResultStore;
        use crate::promise_store::PromiseRepository;
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
            tokio::time::advance(NATS_KV_TIMEOUT + std::time::Duration::from_millis(1)),
        );

        assert_eq!(result.unwrap(), "done despite put timeout");

        // Tool result was NOT persisted — put_tool_result timed out.
        let ck = tool_cache_key("my_tool", &serde_json::json!({}));
        let cached = store.inner.get_tool_result("acme", "p1", &ck).await.unwrap();
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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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
            .run(vec![Message::user_text("caller initial message")], &[], None)
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

    /// When the initial `get_promise` KV read times out, the run must start
    /// fresh from the caller's `initial_messages` rather than blocking.
    ///
    /// `checkpoint` stays `None` so all checkpoint/terminal writes are skipped
    /// for the run, but the run itself completes normally.
    #[tokio::test(start_paused = true)]
    async fn initial_get_promise_timeout_starts_fresh() {
        use crate::promise_store::mock::HangingGetPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(HangingGetPromiseStore::new());

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "fresh run"}]
        });
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));
        let dispatcher = Arc::new(MockToolDispatcher::new("unused"));

        let agent = make_durable_agent(
            anthropic,
            dispatcher,
            Arc::clone(&store) as Arc<dyn PromiseRepository>,
            "p1",
        );

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("initial")], &[], None),
            tokio::time::advance(NATS_KV_TIMEOUT + std::time::Duration::from_millis(1)),
        );

        assert_eq!(
            result.unwrap(),
            "fresh run",
            "run must complete normally when initial KV load times out"
        );
    }

    /// When the initial `get_promise` KV read returns an error, the run must
    /// start fresh from the caller's `initial_messages` rather than returning
    /// the error to the caller.
    #[tokio::test]
    async fn initial_get_promise_error_starts_fresh() {
        use crate::promise_store::mock::ErrorGetPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(ErrorGetPromiseStore::new());

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "fresh run after error"}]
        });
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));
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
            result,
            "fresh run after error",
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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
                    dyn std::future::Future<
                            Output = Result<serde_json::Value, reqwest::Error>,
                        > + Send
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
            system_text,
            "caller fallback prompt",
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::{tool_def, ToolContext, ToolDispatcher};
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
                    dyn std::future::Future<
                            Output = Result<serde_json::Value, reqwest::Error>,
                        > + Send
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
        };

        let result = agent.run(vec![Message::user_text("go")], &[], None).await.unwrap();
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
    /// the Err arm is treated as a cache miss and the tool is re-executed.
    #[tokio::test]
    async fn get_tool_result_error_treats_as_cache_miss_and_reexecutes() {
        use crate::promise_store::mock::ErrorGetToolResultStore;
        use crate::promise_store::PromiseRepository;
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
            "content": [{"type": "text", "text": "done after error"}]
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
        assert_eq!(result, "done after error");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "tool must be re-executed when get_tool_result returns an error"
        );
    }

    // ── MCP tool execution timeout ────────────────────────────────────────────

    /// When an MCP `call_tool` hangs past `TOOL_EXECUTION_TIMEOUT`, the run
    /// must continue and the model must receive a timed-out error string.
    #[tokio::test(start_paused = true)]
    async fn mcp_tool_execution_timeout_returns_error_string() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::{tool_def, ToolContext, ToolDispatcher};
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
        };

        let (result, _) = tokio::join!(
            agent.run(vec![Message::user_text("go")], &[], None),
            tokio::time::advance(TOOL_EXECUTION_TIMEOUT + std::time::Duration::from_secs(1)),
        );

        assert_eq!(result.unwrap(), "done after mcp timeout");

        // Timed-out result must be cached so crash recovery does not re-hang.
        // Cache key uses the prefixed name from the LLM response, not `original`.
        let ck = tool_cache_key("mcp__srv__search", &serde_json::json!({}));
        let cached = store
            .get_tool_result("acme", "p1", &ck)
            .await
            .unwrap()
            .expect("timed-out MCP result must be cached");
        assert!(
            cached.contains("timed out"),
            "cached result must describe the timeout; got: {cached}"
        );
    }

    // ── put_tool_result Err ───────────────────────────────────────────────────

    /// When `put_tool_result` returns an error (not timeout), the run continues
    /// normally — the error is logged but does not abort. The result is not
    /// cached, so a crash would cause re-execution on recovery.
    #[tokio::test]
    async fn put_tool_result_error_run_completes_but_result_not_cached() {
        use crate::promise_store::mock::ErrorPutToolResultStore;
        use crate::promise_store::PromiseRepository;
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

        let result = agent.run(vec![Message::user_text("go")], &[], None).await.unwrap();
        assert_eq!(result, "done despite put error");

        // Result was not persisted — put failed.
        let ck = tool_cache_key("my_tool", &serde_json::json!({}));
        let cached = store.inner.get_tool_result("acme", "p1", &ck).await.unwrap();
        assert!(
            cached.is_none(),
            "tool result must not be cached when put_tool_result returns an error"
        );
    }

    // ── CAS reload get_promise Err → checkpoint = None ────────────────────────

    /// When `get_promise` returns an error during the CAS conflict reload,
    /// `checkpoint` is set to `None` — further checkpointing is disabled but
    /// the run continues to completion normally.
    #[tokio::test]
    async fn cas_reload_get_promise_error_disables_checkpointing_and_run_completes() {
        use crate::promise_store::mock::ErrorReloadStore;
        use crate::promise_store::PromiseRepository;
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

        let result = agent.run(vec![Message::user_text("go")], &[], None).await.unwrap();
        assert_eq!(
            result,
            "done despite reload error",
            "run must complete normally when CAS reload get_promise returns an error"
        );

        // checkpoint=None means terminal Resolved write was skipped.
        let (p, _) = store.inner.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            crate::promise_store::PromiseStatus::Running,
            "promise must remain Running — terminal write skipped when checkpoint=None"
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
        use crate::promise_store::mock::HangingUpdateStore;
        use crate::promise_store::PromiseRepository;
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
        let (p, _) = store.inner.get_promise("acme", "p1").await.unwrap().unwrap();
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
        let input_a =
            serde_json::json!({"outer": {"z": 3, "a": 1, "m": {"y": 9, "x": 7}}});
        let input_b =
            serde_json::json!({"outer": {"a": 1, "m": {"x": 7, "y": 9}, "z": 3}});
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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hello back"}]
        });
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

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
        };

        let (text, messages) = agent
            .run_chat(vec![Message::user_text("hello")], &[], None)
            .await
            .unwrap();

        assert_eq!(text, "hello back");
        // [user("hello"), assistant("hello back")]
        assert_eq!(messages.len(), 2, "expected 2 messages: initial user + assistant end_turn");
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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

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
        assert_eq!(messages[0].role, "user");   // initial
        assert_eq!(messages[1].role, "assistant"); // tool_use
        assert_eq!(messages[2].role, "user");   // tool_result
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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

        // One tool_use response and max_iterations = 1 — loop exhausts immediately.
        let tool_use_resp = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![tool_use_resp]));

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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Mutex;

        struct ErrorClient(Arc<Mutex<Option<reqwest::Error>>>);
        impl AnthropicClient for ErrorClient {
            fn complete<'a>(
                &'a self,
                _body: serde_json::Value,
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>>
            {
                let e = self.0.lock().unwrap().take().expect("error already consumed");
                Box::pin(async move { Err(e) })
            }
        }

        // A URL-parse error gives a real reqwest::Error without a network call.
        let err = reqwest::Client::new()
            .get("not a url at all:///")
            .build()
            .unwrap_err();

        let agent = AgentLoop {
            anthropic_client: Arc::new(ErrorClient(Arc::new(Mutex::new(Some(err))))),
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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

        let malformed = serde_json::json!({"not_a_response": true});
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![malformed]));

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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

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
        use crate::promise_store::mock::HangingUpdateStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(HangingUpdateStore::new());
        store.inner.insert_promise(make_test_promise("p1"));

        // Direct end_turn with no preceding tool_use.  The only update_promise
        // call is the Resolved write inside end_turn.
        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "finished"}]
        });
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

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
        let (p, _) = store.inner.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            crate::promise_store::PromiseStatus::Running,
            "promise must remain Running when the end_turn Resolved write times out"
        );
    }

    /// When `update_promise` returns an error during the `end_turn` Resolved
    /// write, a warning is logged and the run still returns `Ok(text)`.  The
    /// promise status stays `Running` — the Resolved write was never committed.
    ///
    /// Uses `CasConflictOnceStore` with a direct end_turn (no tool_use) so
    /// the only update_promise call is the Resolved write, hitting the
    /// `Ok(Err(e))` arm in the end_turn block.
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
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

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
            "run must return Ok(text) even when the Resolved write returns an error"
        );

        // update_promise returned an error — status was never updated to Resolved.
        let (p, _) = store.inner.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must remain Running when the end_turn Resolved write returns an error"
        );
    }

    // ── end_turn multi-block text join ────────────────────────────────────────

    /// When `end_turn` content contains multiple `Text` blocks they are joined
    /// with `"\n"`.  All existing tests pass a single text block; this covers
    /// the join path in `run()`.
    #[tokio::test]
    async fn run_end_turn_joins_multiple_text_blocks_with_newline() {
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
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
        agent.run(vec![Message::user_text("go")], &[], None).await.unwrap();

        let input = captured.lock().unwrap().take().expect("dispatcher was never called");
        assert!(
            input.get("_idempotency_key").is_none(),
            "_idempotency_key must NOT be injected into non-object tool input; got: {input}"
        );
        assert!(input.is_null(), "non-object input must be forwarded unchanged; got: {input}");
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::mock::MockToolDispatcher;

        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_test_promise("p1"));

        let tool_use_resp = || serde_json::json!({
            "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "tu1", "name": "my_tool", "input": {}}]
        });
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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

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
        let content = vec![ContentBlock::Text { text: "hi".to_string() }];
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

        let store = Arc::new(MockPromiseStore::new());
        // No promise inserted — if the run tried to load one it would get None,
        // but the real check is that it completes without any write activity.

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}]
        });
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

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
        use crate::tools::{mock::MockToolDispatcher, ToolContext};
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
        };

        let result = agent
            .run(vec![Message::user_text("go")], &[], None)
            .await;

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
        let block = ContentBlock::Text { text: "hello world".to_string() };
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
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>>
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
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>>
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
        };

        let result = agent
            .run(vec![Message::user_text("hello")], &[], Some("my system prompt"))
            .await
            .unwrap();

        assert_eq!(result, "reply");

        let body = recording.captured.lock().unwrap().clone().unwrap();
        let system = &body["system"];
        assert!(
            system.is_array(),
            "request must include a system array when system_prompt is Some; got: {body}"
        );
        assert_eq!(system[0]["type"], "text", "system block must have type=text");
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
        assert_eq!(sort_json_keys(&serde_json::json!([])), serde_json::json!([]));
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
        let err = reqwest::Client::new()
            .get("not a url at all:///")
            .build()
            .unwrap_err();
        let display = AgentError::Http(err).to_string();
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
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::{mock::MockToolDispatcher, ToolContext};

        // Empty store — get_promise("acme", "p1") returns Ok(None).
        let store = Arc::new(MockPromiseStore::new());

        let end_turn_resp = serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "fresh start"}]
        });
        let anthropic =
            Arc::new(mock::SequencedMockAnthropicClient::new(vec![end_turn_resp]));

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
            ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, reqwest::Error>> + Send + 'a>>
            {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    let resp = self.first_response.clone();
                    Box::pin(async move { Ok(resp) })
                } else {
                    // Construct a non-retryable error without a real HTTP call.
                    let err = reqwest::Client::new()
                        .get("not a url at all:///")
                        .build()
                        .unwrap_err();
                    Box::pin(async move { Err(err) })
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
        let (p, _) = store.inner.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must stay Running — terminal write is skipped when checkpoint=None"
        );
    }
}
