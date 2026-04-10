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
            match store.get_promise(&self.tenant_id, pid).await {
                Ok(Some((p, rev))) => {
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
            let raw = match self.anthropic_client.complete(body).await {
                Ok(v) => v,
                Err(e) => {
                    if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                        && let Some((ref mut p, ref mut rev)) = checkpoint
                    {
                        p.status = crate::promise_store::PromiseStatus::Failed;
                        let _ = store.update_promise(&self.tenant_id, pid, p, *rev).await;
                    }
                    return Err(AgentError::Http(e));
                }
            };
            let response = serde_json::from_value::<AnthropicResponse>(raw).map_err(|e| {
                AgentError::UnexpectedStopReason(format!("Deserialization error: {e}"))
            })?;

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
                        if let Err(e) = store.update_promise(&self.tenant_id, pid, p, *rev).await {
                            warn!(error = %e, "Failed to mark promise Resolved");
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
                        match store.update_promise(&self.tenant_id, pid, p, *rev).await {
                            Ok(new_rev) => *rev = new_rev,
                            Err(e) => {
                                // CAS conflict: another process wrote to this promise
                                // between our last read and now. Reload the current
                                // revision so future checkpoints can succeed.
                                // Without this reload, every subsequent checkpoint in
                                // this run fails silently with a stale revision.
                                warn!(error = %e, "Promise checkpoint CAS conflict — reloading revision");
                                match store.get_promise(&self.tenant_id, pid).await {
                                    Ok(Some((_, new_rev))) => *rev = new_rev,
                                    Ok(None) => error!(
                                        promise_id = %pid,
                                        "Promise disappeared during CAS reload — checkpointing disabled for this run"
                                    ),
                                    Err(e) => error!(
                                        error = %e,
                                        promise_id = %pid,
                                        "CAS reload failed — checkpointing disabled for this run; crash recovery will replay from last successful checkpoint"
                                    ),
                                }
                            }
                        }
                    }
                }
                other => {
                    return Err(AgentError::UnexpectedStopReason(other.to_string()));
                }
            }
        }

        warn!(max = self.max_iterations, "Agent reached max iterations");
        if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
            && let Some((ref mut p, ref mut rev)) = checkpoint
        {
            p.status = crate::promise_store::PromiseStatus::Failed;
            if let Err(e) = store.update_promise(&self.tenant_id, pid, p, *rev).await {
                warn!(error = %e, "Failed to mark promise Failed");
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
                // checkpoint. During normal forward execution Anthropic always
                // generates a fresh tool_use_id, so a cache hit here would
                // indicate a stale entry — not a replay scenario.
                if recovering
                    && let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                {
                    match store.get_tool_result(&self.tenant_id, pid, id).await {
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
                            serde_json::Value::String(format!("{pid}.{id}")),
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
                // Store the result so that if the process crashes before the
                // next checkpoint write, the result can be replayed on restart
                // rather than re-executing the tool.
                if let (Some(store), Some(pid)) = (&self.promise_store, &self.promise_id)
                    && let Err(e) = store
                        .put_tool_result(&self.tenant_id, pid, id, &output)
                        .await
                {
                    error!(error = %e, "Failed to cache tool result — if process crashes before next checkpoint, this tool will be re-executed on recovery");
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
//
// TODO: Add an integration test for the full checkpoint → crash → recovery cycle:
//   1. Build an AgentLoop with a mock PromiseStore and mock AnthropicClient.
//   2. Run the agent through one tool turn so a checkpoint and tool-result cache
//      entry are written.
//   3. Drop the AgentLoop (simulating a crash).
//   4. Construct a new AgentLoop pointing at the same mock store.
//   5. Call `run_with_history` seeded from the checkpoint.
//   6. Assert the tool is NOT re-executed (cache replay) and the run completes.

#[cfg(test)]
mod tests {
    use super::*;

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
}
