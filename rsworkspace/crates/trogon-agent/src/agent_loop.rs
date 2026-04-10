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
            self.http
                .post(format!("{}/anthropic/v1/messages", self.proxy_url))
                .header("Authorization", format!("Bearer {}", self.anthropic_token))
                .header("anthropic-version", "2023-06-01")
                // Hard cap per LLM call. Without this, a hung Anthropic API keeps
                // the heartbeat alive indefinitely and the run never completes.
                .timeout(std::time::Duration::from_secs(5 * 60))
                .json(&body)
                .send()
                .await?
                .json::<serde_json::Value>()
                .await
        })
    }
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
                        // Transient: NATS redelivery may retry after a network blip.
                        write_promise_terminal(store.as_ref(), &self.tenant_id, pid, p, *rev, crate::promise_store::PromiseStatus::Failed, "HTTP error").await;
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
                        p.messages = messages.clone();
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
                let output = if let Some((_, original, client)) = self
                    .mcp_dispatch
                    .iter()
                    .find(|(prefixed, _, _)| prefixed == name)
                {
                    match client.call_tool(original, &call_input).await {
                        Ok(out) => out,
                        Err(e) => format!("Tool error: {e}"),
                    }
                } else {
                    self.tool_dispatcher.dispatch(name, &call_input).await
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
}
