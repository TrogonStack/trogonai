//! Core agentic loop: prompt → Anthropic (via proxy) → tool calls → repeat.
//!
//! The loop follows the Anthropic tool-use protocol:
//! 1. Send `messages` + `tools` to the model.
//! 2. If `stop_reason == "end_turn"` → return the text output.
//! 3. If `stop_reason == "tool_use"` → execute each requested tool, append
//!    results, and send another request.
//! 4. Repeat until `end_turn` or `max_iterations` is reached.

use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, future::join_all};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::tools::{ToolContext, ToolDef, dispatch_tool};

pub use trogon_tools::{
    ContentBlock, ElicitationProvider, ImageSource, Message, PermissionChecker, ToolResult,
};

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
pub(crate) struct AnthropicResponse {
    pub(crate) stop_reason: String,
    pub(crate) content: Vec<ContentBlock>,
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) usage: Option<AnthropicUsage>,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
pub(crate) struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

// ── AnthropicHttpClient ───────────────────────────────────────────────────────

pub(crate) trait AnthropicHttpClient: Send + Sync + Clone + 'static {
    fn call_anthropic<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
        extra_headers: &'a [(String, String)],
        body: &'a Value,
    ) -> impl std::future::Future<Output = Result<AnthropicResponse, AgentError>> + Send + 'a;
}

impl AnthropicHttpClient for reqwest::Client {
    async fn call_anthropic<'a>(
        &'a self,
        url: &'a str,
        token: &'a str,
        extra_headers: &'a [(String, String)],
        body: &'a Value,
    ) -> Result<AnthropicResponse, AgentError> {
        let mut req_builder = self
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("anthropic-version", "2023-06-01");
        for (k, v) in extra_headers {
            req_builder = req_builder.header(k.as_str(), v.as_str());
        }
        req_builder
            .json(body)
            .send()
            .await
            .map_err(|e| AgentError::Http(HttpError(e.to_string())))?
            .error_for_status()
            .map_err(|e| AgentError::Http(HttpError(e.to_string())))?
            .json::<AnthropicResponse>()
            .await
            .map_err(|e| AgentError::Http(HttpError(e.to_string())))
    }
}

// ── Errors ────────────────────────────────────────────────────────────────────

/// Opaque HTTP error that preserves the error chain without exposing the transport type.
#[derive(Debug)]
pub struct HttpError(String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

#[derive(Debug)]
pub enum AgentError {
    Http(HttpError),
    MaxIterationsReached,
    MaxTokens,
    UnexpectedStopReason(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::MaxIterationsReached => write!(f, "Agent exceeded max iterations"),
            Self::MaxTokens => write!(f, "Context window full (max_tokens)"),
            Self::UnexpectedStopReason(r) => write!(f, "Unexpected stop reason: {r}"),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Http(e) = self { Some(e) } else { None }
    }
}

impl From<reqwest::Error> for AgentError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(HttpError(e.to_string()))
    }
}

// ── AgentEvent ────────────────────────────────────────────────────────────────

/// Events emitted by [`AgentLoop::run_chat_streaming`] during a prompt turn.
///
/// Callers receive these on an `mpsc::Receiver` and can forward them to the
/// client in real time (e.g. as NATS `PromptEvent` messages).
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A chunk of assistant text.
    TextDelta { text: String },
    /// A chunk of the model's internal reasoning (extended thinking).
    ThinkingDelta { text: String },
    /// A tool call was dispatched — emitted immediately before execution.
    ToolCallStarted {
        id: String,
        name: String,
        input: serde_json::Value,
        #[allow(dead_code)]
        parent_tool_use_id: Option<String>,
    },
    /// A tool call completed — emitted immediately after execution.
    ToolCallFinished {
        id: String,
        output: String,
        exit_code: Option<i32>,
        signal: Option<String>,
    },
    /// A system-level status message (forward compatibility with Anthropic API system events).
    SystemStatus { message: String },
    /// Token usage summary emitted at the end of a turn.
    UsageSummary {
        input_tokens: u32,
        output_tokens: u32,
        cache_creation_tokens: u32,
        cache_read_tokens: u32,
    },
}

// ── AnthropicStreamingClient ──────────────────────────────────────────────────

/// Sends a streaming request to the Anthropic messages API and returns the
/// raw byte chunks of the SSE response. Returned stream must be `'static` so
/// it can outlive the call site without holding a borrow on the loop.
pub trait AnthropicStreamingClient: Send + Sync + 'static {
    fn complete_streaming(
        &self,
        body: serde_json::Value,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>;
}

/// Concrete [`AnthropicStreamingClient`] backed by a [`reqwest::Client`].
pub struct ReqwestAnthropicStreamingClient {
    pub http: reqwest::Client,
    pub proxy_url: String,
    pub anthropic_token: String,
    pub anthropic_base_url: Option<String>,
    pub anthropic_extra_headers: Vec<(String, String)>,
}

impl ReqwestAnthropicStreamingClient {
    fn messages_url(&self) -> String {
        if let Some(ref base) = self.anthropic_base_url {
            format!("{base}/messages")
        } else {
            format!("{}/anthropic/v1/messages", self.proxy_url)
        }
    }
}

impl AnthropicStreamingClient for ReqwestAnthropicStreamingClient {
    fn complete_streaming(
        &self,
        mut body: serde_json::Value,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>> {
        body["stream"] = serde_json::json!(true);
        let url = self.messages_url();
        let token = self.anthropic_token.clone();
        let http = self.http.clone();
        let extra_headers = self.anthropic_extra_headers.clone();

        Box::pin(
            futures_util::stream::once(async move {
                let mut req = http
                    .post(&url)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("anthropic-version", "2023-06-01");
                for (k, v) in &extra_headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                req.json(&body).send().await.and_then(|r| r.error_for_status())
            })
            .flat_map(
                |result| -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> {
                    match result {
                        Ok(resp) => Box::pin(resp.bytes_stream()),
                        Err(e) => Box::pin(futures_util::stream::once(std::future::ready(Err(e)))),
                    }
                },
            ),
        )
    }
}

/// [`AnthropicStreamingClient`] that returns a static `end_turn` SSE response
/// without making any network calls. Use it to run the agent loop fully offline.
#[derive(Clone)]
pub struct NoopStreamingClient;

impl AnthropicStreamingClient for NoopStreamingClient {
    fn complete_streaming(
        &self,
        _body: serde_json::Value,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>> {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":0,\"output_tokens\":0,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Model not connected.\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        Box::pin(futures_util::stream::once(std::future::ready(Ok(
            Bytes::from_static(sse.as_bytes()),
        ))))
    }
}

// ── AgentLoop ─────────────────────────────────────────────────────────────────

/// Runs the Anthropic tool-use loop, routing all AI calls through the proxy.
#[derive(Clone)]
pub struct AgentLoop<H = reqwest::Client> {
    pub http_client: H,
    /// Base URL of the running `trogon-secret-proxy`.
    pub proxy_url: String,
    /// Opaque proxy token for Anthropic (never the real API key).
    pub anthropic_token: String,
    /// When set, `run_chat_streaming` dispatches through this trait instead of
    /// building a raw reqwest request. Useful for injecting mocks in tests.
    pub streaming_client: Option<Arc<dyn AnthropicStreamingClient>>,
    /// When set, overrides `proxy_url` as the Anthropic messages base URL.
    /// Format: `https://gateway.example.com/v1` (without trailing `/messages`).
    pub anthropic_base_url: Option<String>,
    /// Additional HTTP headers sent to the Anthropic endpoint (e.g. gateway auth headers).
    pub anthropic_extra_headers: Vec<(String, String)>,
    pub model: String,
    pub max_iterations: u32,
    /// Extended thinking token budget. When `Some(n)` with `n > 0`, the
    /// Anthropic `thinking` feature is enabled with `budget_tokens = n`.
    pub thinking_budget: Option<u32>,
    /// Shared context passed to every tool execution.
    pub tool_context: Arc<ToolContext>,
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
    /// Optional gate called before each tool execution — `None` means all tools are auto-allowed.
    pub permission_checker: Option<Arc<dyn PermissionChecker>>,
    /// Optional provider for the built-in `ask_user` tool; `None` means the tool is not offered.
    pub elicitation_provider: Option<Arc<dyn ElicitationProvider>>,
}

#[allow(private_bounds)]
impl<H: AnthropicHttpClient> AgentLoop<H> {
    /// Build the Anthropic messages API URL, respecting the gateway override.
    fn messages_url(&self) -> String {
        if let Some(ref base) = self.anthropic_base_url {
            format!("{base}/messages")
        } else {
            format!("{}/anthropic/v1/messages", self.proxy_url)
        }
    }

    /// Run the agentic loop starting from `initial_messages`.
    ///
    /// `system_prompt` is injected as the Anthropic `system` field — use it to
    /// provide persistent memory (e.g. the contents of `.trogon/memory.md`).
    /// Pass `None` when no system prompt is needed.
    ///
    /// Returns the final text produced by the model when it stops requesting
    /// tools.
    #[cfg_attr(coverage, coverage(off))]
    pub async fn run(
        &self,
        initial_messages: Vec<Message>,
        tools: &[ToolDef],
        system_prompt: Option<&str>,
    ) -> Result<String, AgentError> {
        let mut messages = initial_messages;

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

            let mut body =
                serde_json::to_value(&request).expect("request serialization is infallible");
            if let Some(budget) = self.thinking_budget
                && budget > 0
            {
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
            }

            let response = self
                .http_client
                .call_anthropic(
                    &self.messages_url(),
                    &self.anthropic_token,
                    &self.anthropic_extra_headers,
                    &body,
                )
                .await?;

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
                    return Ok(text);
                }
                "max_tokens" => {
                    warn!(iteration, "Agent hit max_tokens (context full)");
                    return Err(AgentError::MaxTokens);
                }
                "tool_use" => {
                    let results = self.execute_tools(&response.content).await;
                    messages.push(Message::assistant(response.content));
                    messages.push(Message::tool_results(results));
                }
                other => {
                    return Err(AgentError::UnexpectedStopReason(other.to_string()));
                }
            }
        }

        warn!(max = self.max_iterations, "Agent reached max iterations");
        Err(AgentError::MaxIterationsReached)
    }

    /// Like [`run`] but also returns the full updated message history.
    ///
    /// Used by the interactive chat API to persist conversation across turns.
    /// `initial_messages` should contain the prior history; the returned
    /// `Vec<Message>` is that history extended with the new user turn, all
    /// intermediate tool exchanges, and the final assistant turn.
    #[cfg_attr(coverage, coverage(off))]
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

            let mut body =
                serde_json::to_value(&request).expect("request serialization is infallible");
            if let Some(budget) = self.thinking_budget
                && budget > 0
            {
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
            }

            let response = self
                .http_client
                .call_anthropic(
                    &self.messages_url(),
                    &self.anthropic_token,
                    &self.anthropic_extra_headers,
                    &body,
                )
                .await?;

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
                "max_tokens" => {
                    warn!(iteration, "Chat hit max_tokens (context full)");
                    return Err(AgentError::MaxTokens);
                }
                "tool_use" => {
                    let results = self.execute_tools(&response.content).await;
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

    /// Like [`run_chat`] but emits [`AgentEvent`]s on `event_tx` throughout execution.
    ///
    /// - `TextDelta` is emitted when the model produces text at `end_turn`.
    /// - `ToolCallStarted` is emitted for each tool call before it runs.
    /// - `ToolCallFinished` is emitted for each tool call after it completes.
    ///
    /// Returns the updated message history (same as [`run_chat`]).
    /// Errors on `event_tx` are swallowed — the receiver dropping does not abort the loop.
    #[cfg_attr(coverage, coverage(off))]
    pub async fn run_chat_streaming(
        &self,
        initial_messages: Vec<Message>,
        tools: &[ToolDef],
        system_prompt: Option<&str>,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
        mut steer_rx: Option<tokio::sync::mpsc::Receiver<String>>,
    ) -> Result<Vec<Message>, AgentError>
    {
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

        let mut total_input: u32 = 0;
        let mut total_output: u32 = 0;
        let mut total_cache_creation: u32 = 0;
        let mut total_cache_read: u32 = 0;

        for iteration in 0..self.max_iterations {
            debug!(iteration, "Streaming chat loop iteration");

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

            let mut body =
                serde_json::to_value(&request).expect("request serialization is infallible");
            if let Some(budget) = self.thinking_budget
                && budget > 0
            {
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
            }

            // ── Obtain the SSE byte stream ────────────────────────────────────
            // Build a concrete client from the loop's current fields when no
            // mock is injected. This means apply_gateway() mutations are always
            // picked up — the client is never stale.
            let built_client: Arc<dyn AnthropicStreamingClient>;
            let streaming: &dyn AnthropicStreamingClient = match &self.streaming_client {
                Some(c) => c.as_ref(),
                None => {
                    built_client = Arc::new(ReqwestAnthropicStreamingClient {
                        http: reqwest::Client::new(),
                        proxy_url: self.proxy_url.clone(),
                        anthropic_token: self.anthropic_token.clone(),
                        anthropic_base_url: self.anthropic_base_url.clone(),
                        anthropic_extra_headers: self.anthropic_extra_headers.clone(),
                    });
                    built_client.as_ref()
                }
            };
            let mut byte_stream = streaming.complete_streaming(body);

            // ── Parse SSE stream ──────────────────────────────────────────────
            let mut sse = SseParser::new();

            let mut stop_reason = String::new();
            let mut block_builders: Vec<Option<ContentBlockBuilder>> = Vec::new();
            let mut turn_input = 0u32;
            let mut turn_output = 0u32;
            let mut turn_cache_creation = 0u32;
            let mut turn_cache_read = 0u32;

            while let Some(chunk_result) = byte_stream.next().await {
                let bytes = chunk_result.map_err(AgentError::from)?;

                for (event_type, data) in sse.feed(&bytes) {
                    match event_type.as_str() {
                        "message_start" => {
                            let u = &data["message"]["usage"];
                            turn_input = u["input_tokens"].as_u64().unwrap_or(0) as u32;
                            turn_cache_creation = u["cache_creation_input_tokens"].as_u64().unwrap_or(0) as u32;
                            turn_cache_read = u["cache_read_input_tokens"].as_u64().unwrap_or(0) as u32;
                        }
                        "content_block_start" => {
                            let idx = data["index"].as_u64().unwrap_or(0) as usize;
                            while block_builders.len() <= idx {
                                block_builders.push(None);
                            }
                            let cb = &data["content_block"];
                            block_builders[idx] = match cb["type"].as_str() {
                                Some("text") => Some(ContentBlockBuilder::Text(String::new())),
                                Some("thinking") => Some(ContentBlockBuilder::Thinking(String::new())),
                                Some("tool_use") => {
                                    let id = cb["id"].as_str().unwrap_or("").to_string();
                                    let name = cb["name"].as_str().unwrap_or("").to_string();
                                    let parent_tool_use_id = cb["parent_tool_use_id"].as_str().map(String::from);
                                    Some(ContentBlockBuilder::ToolUse { id, name, input_buf: String::new(), parent_tool_use_id })
                                }
                                _ => None,
                            };
                        }
                        "content_block_delta" => {
                            let idx = data["index"].as_u64().unwrap_or(0) as usize;
                            let delta = &data["delta"];
                            if let Some(Some(builder)) = block_builders.get_mut(idx) {
                                match (delta["type"].as_str(), builder) {
                                    (Some("text_delta"), ContentBlockBuilder::Text(t)) => {
                                        if let Some(text) = delta["text"].as_str() {
                                            t.push_str(text);
                                            let _ = event_tx
                                                .send(AgentEvent::TextDelta { text: text.to_string() })
                                                .await;
                                        }
                                    }
                                    (Some("thinking_delta"), ContentBlockBuilder::Thinking(t)) => {
                                        if let Some(thinking) = delta["thinking"].as_str() {
                                            t.push_str(thinking);
                                            let _ = event_tx
                                                .send(AgentEvent::ThinkingDelta { text: thinking.to_string() })
                                                .await;
                                        }
                                    }
                                    (Some("input_json_delta"), ContentBlockBuilder::ToolUse { input_buf, .. }) => {
                                        if let Some(partial) = delta["partial_json"].as_str() {
                                            input_buf.push_str(partial);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "message_delta" => {
                            stop_reason = data["delta"]["stop_reason"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            turn_output = data["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
                        }
                        _ => {}
                    }
                }
            }

            // Reconstruct the content blocks from builders.
            let response_content: Vec<ContentBlock> = block_builders
                .into_iter()
                .filter_map(|opt| match opt? {
                    ContentBlockBuilder::Text(t) => Some(ContentBlock::Text { text: t }),
                    ContentBlockBuilder::Thinking(t) => Some(ContentBlock::Thinking { thinking: t }),
                    ContentBlockBuilder::ToolUse { id, name, input_buf, parent_tool_use_id } => {
                        let input = serde_json::from_str(&input_buf)
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        Some(ContentBlock::ToolUse { id, name, input, parent_tool_use_id })
                    }
                })
                .collect();

            // Accumulate per-turn usage into session totals.
            total_input = total_input.saturating_add(turn_input);
            total_output = total_output.saturating_add(turn_output);
            total_cache_creation = total_cache_creation.saturating_add(turn_cache_creation);
            total_cache_read = total_cache_read.saturating_add(turn_cache_read);

            match stop_reason.as_str() {
                "end_turn" => {
                    // TextDelta and ThinkingDelta were already emitted incrementally above.
                    let _ = event_tx
                        .send(AgentEvent::UsageSummary {
                            input_tokens: total_input,
                            output_tokens: total_output,
                            cache_creation_tokens: total_cache_creation,
                            cache_read_tokens: total_cache_read,
                        })
                        .await;

                    messages.push(Message::assistant(response_content));
                    info!(iterations = iteration + 1, "Streaming chat completed");
                    return Ok(messages);
                }
                "max_tokens" => {
                    let _ = event_tx
                        .send(AgentEvent::UsageSummary {
                            input_tokens: total_input,
                            output_tokens: total_output,
                            cache_creation_tokens: total_cache_creation,
                            cache_read_tokens: total_cache_read,
                        })
                        .await;
                    // Text was already emitted incrementally during the SSE parse above.
                    warn!(iteration, "Streaming chat hit max_tokens (context full)");
                    return Err(AgentError::MaxTokens);
                }
                "tool_use" => {
                    let results = self
                        .execute_tools_streaming(&response_content, &event_tx)
                        .await;
                    messages.push(Message::assistant(response_content));
                    let mut tool_results_msg = Message::tool_results(results);
                    if let Some(ref mut rx) = steer_rx {
                        while let Ok(text) = rx.try_recv() {
                            tool_results_msg.content.push(ContentBlock::Text { text });
                        }
                    }
                    messages.push(tool_results_msg);
                }
                other => {
                    return Err(AgentError::UnexpectedStopReason(other.to_string()));
                }
            }
        }

        warn!(
            max = self.max_iterations,
            "Streaming chat reached max iterations"
        );
        Err(AgentError::MaxIterationsReached)
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn execute_tools_streaming(
        &self,
        content: &[ContentBlock],
        event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Vec<ToolResult> {
        join_all(content.iter().filter_map(|block| {
            let ContentBlock::ToolUse { id, name, input, parent_tool_use_id } = block else {
                return None;
            };
            Some(self.run_tool_streaming(id, name, input, parent_tool_use_id.as_deref(), event_tx))
        }))
        .await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn execute_tools(&self, content: &[ContentBlock]) -> Vec<ToolResult> {
        join_all(content.iter().filter_map(|block| {
            let ContentBlock::ToolUse { id, name, input, .. } = block else {
                return None;
            };
            Some(self.run_tool(id, name, input))
        }))
        .await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn run_tool_streaming(
        &self,
        id: &str,
        name: &str,
        input: &Value,
        parent_tool_use_id: Option<&str>,
        event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> ToolResult {
        debug!(tool = %name, "Executing tool (streaming)");
        let _ = event_tx
            .send(AgentEvent::ToolCallStarted {
                id: id.to_owned(),
                name: name.to_owned(),
                input: input.clone(),
                parent_tool_use_id: parent_tool_use_id.map(String::from),
            })
            .await;
        let output = self.tool_output(id, name, input).await;
        let _ = event_tx
            .send(AgentEvent::ToolCallFinished {
                id: id.to_owned(),
                output: output.clone(),
                exit_code: None,
                signal: None,
            })
            .await;
        ToolResult { tool_use_id: id.to_owned(), content: output }
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn run_tool(&self, id: &str, name: &str, input: &Value) -> ToolResult {
        debug!(tool = %name, "Executing tool");
        let output = self.tool_output(id, name, input).await;
        ToolResult { tool_use_id: id.to_owned(), content: output }
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn tool_output(&self, id: &str, name: &str, input: &Value) -> String {
        if name == "ask_user" {
            let question = input.get("question").and_then(|v| v.as_str()).unwrap_or("");
            match &self.elicitation_provider {
                Some(provider) => match provider.elicit(question).await {
                    Some(answer) => answer,
                    None => "The user declined or cancelled the request.".to_string(),
                },
                None => "ask_user tool is not available in this context.".to_string(),
            }
        } else {
            let allowed = match &self.permission_checker {
                Some(checker) => checker.check(id, name, input).await,
                None => true,
            };
            if !allowed {
                format!("Permission denied: user refused to run tool `{name}`")
            } else if let Some((_, original, client)) =
                self.mcp_dispatch.iter().find(|(prefixed, _, _)| prefixed == name)
            {
                match client.call_tool(original, input).await {
                    Ok(out) => out,
                    Err(e) => format!("Tool error: {e}"),
                }
            } else {
                dispatch_tool(&self.tool_context, name, input).await
            }
        }
    }
}

// ── SSE streaming helpers ─────────────────────────────────────────────────────

/// Incremental SSE parser.  Feed raw bytes and receive parsed `(event_type,
/// data_json)` pairs.  Handles SSE streams split across multiple network chunks.
struct SseParser {
    buf: String,
}

impl SseParser {
    fn new() -> Self {
        Self { buf: String::new() }
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<(String, serde_json::Value)> {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return vec![];
        };
        self.buf.push_str(text);

        let mut events = Vec::new();
        while let Some(pos) = self.buf.find("\n\n") {
            let raw = self.buf[..pos].to_string();
            self.buf = self.buf[pos + 2..].to_string();

            let mut event_type = String::new();
            let mut data = String::new();
            for line in raw.lines() {
                if let Some(val) = line.strip_prefix("event: ") {
                    event_type = val.to_string();
                } else if let Some(val) = line.strip_prefix("data: ") {
                    data = val.to_string();
                }
            }

            if !data.is_empty()
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                events.push((event_type, v));
            }
        }
        events
    }
}

/// Accumulator for a single content block during SSE streaming.
enum ContentBlockBuilder {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        input_buf: String,
        parent_tool_use_id: Option<String>,
    },
}

// ── SSE parser tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod sse_parser_tests {
    use super::SseParser;

    #[test]
    fn single_complete_event_is_parsed() {
        let mut p = SseParser::new();
        let events = p.feed(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "content_block_delta");
    }

    #[test]
    fn event_split_across_two_chunks_is_assembled() {
        let mut p = SseParser::new();
        let first = p.feed(b"event: message_start\ndata: {\"type\":");
        assert!(first.is_empty(), "incomplete event must not be returned early");
        let second = p.feed(b"\"message_start\"}\n\n");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].0, "message_start");
    }

    #[test]
    fn two_events_in_one_chunk() {
        let mut p = SseParser::new();
        let events = p.feed(
            b"event: ping\ndata: {\"type\":\"ping\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "ping");
        assert_eq!(events[1].0, "message_stop");
    }

    #[test]
    fn non_json_data_line_is_skipped_silently() {
        let mut p = SseParser::new();
        let events = p.feed(b"event: ping\ndata: not-valid-json\n\n");
        assert!(events.is_empty(), "invalid JSON data must be silently dropped");
    }

    #[test]
    fn event_without_data_line_is_skipped() {
        let mut p = SseParser::new();
        let events = p.feed(b"event: ping\n\n");
        assert!(events.is_empty());
    }

    #[test]
    fn text_delta_value_is_accessible() {
        let mut p = SseParser::new();
        let events = p.feed(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        );
        assert_eq!(events.len(), 1);
        let text = events[0].1["delta"]["text"].as_str().unwrap();
        assert_eq!(text, "Hello");
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── MockAnthropicClient ───────────────────────────────────────────────────

    #[derive(Clone)]
    #[allow(dead_code)]
    struct MockAnthropicClient {
        response_json: String,
    }

    #[allow(dead_code)]
    impl MockAnthropicClient {
        fn with_response(json: impl Into<String>) -> Self {
            Self { response_json: json.into() }
        }
    }

    impl AnthropicHttpClient for MockAnthropicClient {
        fn call_anthropic<'a>(
            &'a self,
            _url: &'a str,
            _token: &'a str,
            _extra_headers: &'a [(String, String)],
            _body: &'a Value,
        ) -> impl std::future::Future<Output = Result<AnthropicResponse, AgentError>> + Send + 'a {
            let resp: AnthropicResponse = serde_json::from_str(&self.response_json)
                .expect("test fixture must be valid JSON");
            async move { Ok(resp) }
        }
    }

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
        assert!(AgentError::MaxTokens.to_string().contains("max_tokens"));
        assert!(AgentError::Http(HttpError("connection refused".into())).to_string().contains("HTTP error"));
    }

    #[test]
    fn agent_error_source_for_http_variant() {
        let agent_err = AgentError::Http(HttpError("connect error".into()));
        assert!(std::error::Error::source(&agent_err).is_some());
    }

    #[test]
    fn agent_error_source_none_for_non_http() {
        assert!(std::error::Error::source(&AgentError::MaxIterationsReached).is_none());
        assert!(std::error::Error::source(&AgentError::MaxTokens).is_none());
        assert!(std::error::Error::source(&AgentError::UnexpectedStopReason("x".into())).is_none());
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
        let cached_tools: Vec<crate::tools::ToolDef> = vec![];
        // last_mut() returns None on an empty vec — no panic, no cache_control set.
        assert!(cached_tools.last().is_none());
        assert!(cached_tools.is_empty());
    }

    fn make_test_agent() -> AgentLoop {
        use crate::tools::ToolContext;
        let http_client = reqwest::Client::new();
        let tool_context = Arc::new(ToolContext {
            http_client: http_client.clone(),
            proxy_url: "http://unused:9999".to_string(),
            cwd: ".".to_string(),
        });
        AgentLoop {
            http_client,
            proxy_url: "http://unused:9999".to_string(),
            anthropic_token: "test".to_string(),
            anthropic_base_url: None,
            anthropic_extra_headers: vec![],
            streaming_client: None,
            model: "claude-opus-4-6".to_string(),
            max_iterations: 1,
            thinking_budget: None,
            tool_context,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            permission_checker: None,
            elicitation_provider: None,
        }
    }


    /// Covers line 706: closing `}` of the if-let in execute_tools_streaming
    /// when content contains a ToolUse block with no matching MCP dispatch entry.
    #[tokio::test]
    async fn execute_tools_streaming_with_tool_use_uses_dispatch_tool() {
        let agent = make_test_agent();
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "some_tool".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Unknown tool"));
    }

    /// Covers line 737: closing `}` of the if-let in execute_tools
    /// when content contains a ToolUse block with no matching MCP dispatch entry.
    #[tokio::test]
    async fn execute_tools_with_tool_use_uses_dispatch_tool() {
        let agent = make_test_agent();
        let content = vec![ContentBlock::ToolUse {
            id: "t2".to_string(),
            name: "my_tool".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools(&content).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Unknown tool"));
    }

    /// Covers MCP Ok arm in execute_tools_streaming.
    #[tokio::test]
    async fn execute_tools_streaming_mcp_ok_covers_ok_arm() {
        let mock = Arc::new(trogon_mcp::MockMcpClient::new());
        mock.set_response("mcp ok");
        let mut agent = make_test_agent();
        agent.mcp_dispatch = vec![(
            "srv__tool".to_string(),
            "tool".to_string(),
            mock as Arc<dyn trogon_mcp::McpCallTool>,
        )];
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "m1".to_string(),
            name: "srv__tool".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;
        assert_eq!(results[0].content, "mcp ok");
    }

    /// Covers MCP Err arm in execute_tools_streaming.
    #[tokio::test]
    async fn execute_tools_streaming_mcp_err_covers_err_arm() {
        let mock = Arc::new(trogon_mcp::MockMcpClient::new());
        mock.set_error("tool failed");
        let mut agent = make_test_agent();
        agent.mcp_dispatch = vec![(
            "srv__tool2".to_string(),
            "tool2".to_string(),
            mock as Arc<dyn trogon_mcp::McpCallTool>,
        )];
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "m2".to_string(),
            name: "srv__tool2".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;
        assert!(results[0].content.contains("Tool error"));
    }

    /// Covers MCP Ok arm in execute_tools.
    #[tokio::test]
    async fn execute_tools_mcp_ok_covers_ok_arm() {
        let mock = Arc::new(trogon_mcp::MockMcpClient::new());
        mock.set_response("sync ok");
        let mut agent = make_test_agent();
        agent.mcp_dispatch = vec![(
            "s__t".to_string(),
            "t".to_string(),
            mock as Arc<dyn trogon_mcp::McpCallTool>,
        )];
        let content = vec![ContentBlock::ToolUse {
            id: "m3".to_string(),
            name: "s__t".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools(&content).await;
        assert_eq!(results[0].content, "sync ok");
    }

    /// Covers MCP Err arm in execute_tools.
    #[tokio::test]
    async fn execute_tools_mcp_err_covers_err_arm() {
        let mock = Arc::new(trogon_mcp::MockMcpClient::new());
        mock.set_error("sync fail");
        let mut agent = make_test_agent();
        agent.mcp_dispatch = vec![(
            "s__t2".to_string(),
            "t2".to_string(),
            mock as Arc<dyn trogon_mcp::McpCallTool>,
        )];
        let content = vec![ContentBlock::ToolUse {
            id: "m4".to_string(),
            name: "s__t2".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools(&content).await;
        assert!(results[0].content.contains("Tool error"));
    }

    /// Covers: TextDelta emitted in the max_tokens path when text is non-empty.
    #[tokio::test]
    async fn run_chat_streaming_max_tokens_with_text_emits_text_delta() {
        struct MaxTokensMock;
        impl AnthropicStreamingClient for MaxTokensMock {
            fn complete_streaming(
                &self,
                _body: serde_json::Value,
            ) -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>> {
                let sse: &'static [u8] = b"\
event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":0,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":1}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
                Box::pin(futures_util::stream::once(std::future::ready(Ok(Bytes::from_static(sse)))))
            }
        }

        let mut agent = make_test_agent();
        agent.streaming_client = Some(Arc::new(MaxTokensMock));

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let result = agent
            .run_chat_streaming(vec![Message::user_text("hello")], &[], None, tx, None)
            .await;
        assert!(result.is_err());
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "partial")),
            "expected TextDelta with 'partial' text, got: {events:?}"
        );
    }

    /// Injects a `MockAnthropicStreamingClient` and asserts that
    /// `run_chat_streaming` emits `TextDelta` events without any real HTTP server.
    #[tokio::test]
    async fn run_chat_streaming_uses_streaming_client_trait() {
        struct MockStreamingClient {
            sse: &'static [u8],
        }
        impl AnthropicStreamingClient for MockStreamingClient {
            fn complete_streaming(
                &self,
                _body: serde_json::Value,
            ) -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>> {
                let data = Bytes::from_static(self.sse);
                Box::pin(futures_util::stream::once(std::future::ready(Ok(data))))
            }
        }

        let sse: &'static [u8] = b"\
event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":0,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

        let mut agent = make_test_agent();
        agent.streaming_client = Some(Arc::new(MockStreamingClient { sse }));

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let result = agent
            .run_chat_streaming(vec![Message::user_text("hello")], &[], None, tx, None)
            .await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "hi")),
            "expected TextDelta('hi'), got {events:?}"
        );
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

    /// When `elicitation_provider` returns `Some(answer)`, `execute_tools_streaming`
    /// must return that answer as the tool result content.
    #[tokio::test]
    async fn ask_user_with_provider_returning_answer_uses_that_answer() {
        let mut agent = make_test_agent();
        agent.elicitation_provider =
            Some(Arc::new(ConstElicitation(Some("42".to_string()))) as Arc<dyn ElicitationProvider>);

        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "u1".to_string(),
            name: "ask_user".to_string(),
            input: serde_json::json!({ "question": "What is the answer?" }),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "42");
    }

    /// When `elicitation_provider` returns `None` (user declined), `execute_tools_streaming`
    /// must return the "declined" fallback string.
    #[tokio::test]
    async fn ask_user_with_provider_returning_none_uses_declined_message() {
        let mut agent = make_test_agent();
        agent.elicitation_provider =
            Some(Arc::new(ConstElicitation(None)) as Arc<dyn ElicitationProvider>);

        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "u2".to_string(),
            name: "ask_user".to_string(),
            input: serde_json::json!({ "question": "Continue?" }),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;

        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("declined") || results[0].content.contains("cancelled"),
            "expected declined/cancelled message, got: {}",
            results[0].content
        );
    }

    /// When `elicitation_provider` is `None`, `execute_tools_streaming` must
    /// return the "not available" fallback string for `ask_user` tool calls.
    #[tokio::test]
    async fn ask_user_without_provider_returns_not_available_message() {
        let agent = make_test_agent(); // elicitation_provider: None

        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "u3".to_string(),
            name: "ask_user".to_string(),
            input: serde_json::json!({ "question": "What now?" }),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;

        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("not available"),
            "expected 'not available' message, got: {}",
            results[0].content
        );
    }

    // ── PermissionChecker deny ────────────────────────────────────────────────

    struct DenyAllChecker;

    impl PermissionChecker for DenyAllChecker {
        fn check<'a>(
            &'a self,
            _id: &'a str,
            _name: &'a str,
            _input: &'a serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
            Box::pin(std::future::ready(false))
        }
    }

    #[tokio::test]
    async fn execute_tools_permission_denied_returns_denied_message() {
        let mut agent = make_test_agent();
        agent.permission_checker = Some(Arc::new(DenyAllChecker));
        let content = vec![ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "foo.txt"}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools(&content).await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("Permission denied"),
            "got: {}",
            results[0].content
        );
    }

    #[tokio::test]
    async fn execute_tools_streaming_permission_denied_returns_denied_message() {
        let mut agent = make_test_agent();
        agent.permission_checker = Some(Arc::new(DenyAllChecker));
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "t2".to_string(),
            name: "write_file".to_string(),
            input: serde_json::json!({"path": "out.txt", "content": "x"}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("Permission denied"),
            "got: {}",
            results[0].content
        );
    }

    // ── execute_tools (non-streaming) ask_user ────────────────────────────────

    #[tokio::test]
    async fn execute_tools_ask_user_with_provider_returning_answer() {
        let mut agent = make_test_agent();
        agent.elicitation_provider =
            Some(Arc::new(ConstElicitation(Some("my answer".to_string()))) as Arc<dyn ElicitationProvider>);
        let content = vec![ContentBlock::ToolUse {
            id: "u1".to_string(),
            name: "ask_user".to_string(),
            input: serde_json::json!({"question": "What is X?"}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools(&content).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "my answer");
    }

    #[tokio::test]
    async fn execute_tools_ask_user_with_provider_returning_none() {
        let mut agent = make_test_agent();
        agent.elicitation_provider =
            Some(Arc::new(ConstElicitation(None)) as Arc<dyn ElicitationProvider>);
        let content = vec![ContentBlock::ToolUse {
            id: "u2".to_string(),
            name: "ask_user".to_string(),
            input: serde_json::json!({"question": "Continue?"}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools(&content).await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("declined") || results[0].content.contains("cancelled"),
            "expected declined/cancelled, got: {}",
            results[0].content
        );
    }

    #[tokio::test]
    async fn execute_tools_ask_user_without_provider_returns_not_available() {
        let agent = make_test_agent();
        let content = vec![ContentBlock::ToolUse {
            id: "u3".to_string(),
            name: "ask_user".to_string(),
            input: serde_json::json!({"question": "What now?"}),
            parent_tool_use_id: None,
        }];
        let results = agent.execute_tools(&content).await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.contains("not available"),
            "expected 'not available', got: {}",
            results[0].content
        );
    }

    // ── messages_url ──────────────────────────────────────────────────────────

    #[test]
    fn messages_url_uses_proxy_when_no_base_url() {
        let agent = make_test_agent(); // anthropic_base_url: None
        let url = agent.messages_url();
        assert!(url.contains("http://unused:9999"), "got: {url}");
        assert!(url.ends_with("/anthropic/v1/messages"), "got: {url}");
    }

    #[test]
    fn messages_url_uses_base_url_when_set() {
        let mut agent = make_test_agent();
        agent.anthropic_base_url = Some("https://gateway.example.com/v1".to_string());
        let url = agent.messages_url();
        assert_eq!(url, "https://gateway.example.com/v1/messages");
    }

    /// Proves TextDelta events are emitted as SSE bytes arrive, not buffered
    /// until the full stream closes. A DelayedStreamingClient delivers the
    /// text-delta chunk immediately and holds back the closing chunk for 100ms.
    /// At the 50ms mark the receiver must already contain TextDelta("chunk1").
    #[tokio::test]
    async fn run_chat_streaming_emits_text_delta_incrementally() {
        use std::future::ready;
        use std::time::Duration;
        use tokio::time::sleep;

        struct DelayedStreamingClient;
        impl AnthropicStreamingClient for DelayedStreamingClient {
            fn complete_streaming(
                &self,
                _body: serde_json::Value,
            ) -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>> {
                let chunk1 = Bytes::from(concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}\n\n",
                    "event: content_block_start\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                    "event: content_block_delta\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"chunk1\"}}\n\n",
                ));
                let chunk2 = Bytes::from(concat!(
                    "event: content_block_stop\n",
                    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                    "event: message_delta\n",
                    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                ));
                let stream = futures_util::stream::once(ready(Ok::<Bytes, reqwest::Error>(chunk1)))
                    .chain(futures_util::stream::once(async move {
                        sleep(Duration::from_millis(100)).await;
                        Ok::<Bytes, reqwest::Error>(chunk2)
                    }));
                Box::pin(stream)
            }
        }

        let mut agent = make_test_agent();
        agent.streaming_client = Some(Arc::new(DelayedStreamingClient));

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let handle = tokio::spawn(async move {
            agent
                .run_chat_streaming(vec![Message::user_text("hello")], &[], None, tx, None)
                .await
        });

        sleep(Duration::from_millis(50)).await;
        let early_events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            early_events
                .iter()
                .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "chunk1")),
            "TextDelta('chunk1') must arrive before stream closes; got {early_events:?}"
        );

        let result = handle.await.expect("task panicked");
        assert!(result.is_ok(), "run_chat_streaming failed: {result:?}");
    }

    // ── AgentEvent ────────────────────────────────────────────────────────────

    #[test]
    fn agent_event_text_delta_is_constructible_and_cloneable() {
        let ev = AgentEvent::TextDelta { text: "hello".to_string() };
        let cloned = ev.clone();
        assert!(matches!(cloned, AgentEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn agent_event_thinking_delta_is_constructible_and_cloneable() {
        let ev = AgentEvent::ThinkingDelta { text: "reasoning...".to_string() };
        let cloned = ev.clone();
        assert!(matches!(cloned, AgentEvent::ThinkingDelta { text } if text == "reasoning..."));
    }

    #[test]
    fn agent_event_tool_call_started_fields_are_accessible() {
        let input = serde_json::json!({"key": "val"});
        let ev = AgentEvent::ToolCallStarted {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            input: input.clone(),
            parent_tool_use_id: Some("parent-42".to_string()),
        };
        let cloned = ev.clone();
        match cloned {
            AgentEvent::ToolCallStarted { id, name, input: inp, parent_tool_use_id } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "bash");
                assert_eq!(inp, input);
                assert_eq!(parent_tool_use_id.as_deref(), Some("parent-42"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn agent_event_tool_call_started_without_parent() {
        let ev = AgentEvent::ToolCallStarted {
            id: "c".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({}),
            parent_tool_use_id: None,
        };
        assert!(matches!(
            ev,
            AgentEvent::ToolCallStarted { parent_tool_use_id: None, .. }
        ));
    }

    #[test]
    fn agent_event_tool_call_finished_fields_are_accessible() {
        let ev = AgentEvent::ToolCallFinished {
            id: "call-2".to_string(),
            output: "result text".to_string(),
            exit_code: Some(0),
            signal: None,
        };
        let cloned = ev.clone();
        match cloned {
            AgentEvent::ToolCallFinished { id, output, exit_code, signal } => {
                assert_eq!(id, "call-2");
                assert_eq!(output, "result text");
                assert_eq!(exit_code, Some(0));
                assert!(signal.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn agent_event_tool_call_finished_with_signal() {
        let ev = AgentEvent::ToolCallFinished {
            id: "c".to_string(),
            output: String::new(),
            exit_code: None,
            signal: Some("SIGKILL".to_string()),
        };
        assert!(matches!(
            ev,
            AgentEvent::ToolCallFinished { signal: Some(s), .. } if s == "SIGKILL"
        ));
    }

    #[test]
    fn agent_event_system_status_is_constructible_and_cloneable() {
        let ev = AgentEvent::SystemStatus { message: "overloaded".to_string() };
        let cloned = ev.clone();
        assert!(matches!(cloned, AgentEvent::SystemStatus { message } if message == "overloaded"));
    }

    #[test]
    fn agent_event_usage_summary_fields_are_accessible() {
        let ev = AgentEvent::UsageSummary {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 5,
        };
        let cloned = ev.clone();
        match cloned {
            AgentEvent::UsageSummary {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            } => {
                assert_eq!(input_tokens, 100);
                assert_eq!(output_tokens, 50);
                assert_eq!(cache_creation_tokens, 10);
                assert_eq!(cache_read_tokens, 5);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn agent_event_debug_format_is_non_empty() {
        let events = vec![
            AgentEvent::TextDelta { text: "t".to_string() },
            AgentEvent::ThinkingDelta { text: "th".to_string() },
            AgentEvent::ToolCallStarted {
                id: "i".to_string(),
                name: "n".to_string(),
                input: serde_json::json!({}),
                parent_tool_use_id: None,
            },
            AgentEvent::ToolCallFinished {
                id: "i".to_string(),
                output: "o".to_string(),
                exit_code: None,
                signal: None,
            },
            AgentEvent::SystemStatus { message: "m".to_string() },
            AgentEvent::UsageSummary {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        ];
        for ev in events {
            let dbg = format!("{ev:?}");
            assert!(!dbg.is_empty(), "Debug output must be non-empty");
        }
    }
}
