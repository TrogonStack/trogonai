//! Core agentic loop: prompt → Anthropic (via proxy) → tool calls → repeat.
//!
//! The loop follows the Anthropic tool-use protocol:
//! 1. Send `messages` + `tools` to the model.
//! 2. If `stop_reason == "end_turn"` → return the text output.
//! 3. If `stop_reason == "tool_use"` → execute each requested tool, append
//!    results, and send another request.
//! 4. Repeat until `end_turn` or `max_iterations` is reached.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::tools::{ToolContext, ToolDef, dispatch_tool};

// ── PermissionChecker ─────────────────────────────────────────────────────────

/// Called by the agent loop before each tool execution.
/// Returns `true` to allow the tool to run, `false` to deny it.
pub trait PermissionChecker: Send + Sync {
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>>;
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

/// Source for an image content block sent to the Anthropic API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Base64-encoded image data.
    Base64 { media_type: String, data: String },
    /// Remote image URL.
    Url { url: String },
}

/// A single block within a message's `content` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text from the model or the user.
    Text { text: String },
    /// Image sent by the user (base64 or URL).
    Image { source: ImageSource },
    /// Extended thinking block produced by the model (requires thinking beta).
    Thinking { thinking: String },
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
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AgentError {
    Http(reqwest::Error),
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
        if let Self::Http(e) = self {
            Some(e)
        } else {
            None
        }
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

// ── AgentLoop ─────────────────────────────────────────────────────────────────

/// Runs the Anthropic tool-use loop, routing all AI calls through the proxy.
#[derive(Clone)]
pub struct AgentLoop {
    pub http_client: reqwest::Client,
    /// Base URL of the running `trogon-secret-proxy`.
    pub proxy_url: String,
    /// Opaque proxy token for Anthropic (never the real API key).
    pub anthropic_token: String,
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
    pub mcp_dispatch: Vec<(String, String, Arc<trogon_mcp::McpClient>)>,
    /// Optional gate called before each tool execution — `None` means all tools are auto-allowed.
    pub permission_checker: Option<Arc<dyn PermissionChecker>>,
}

impl AgentLoop {
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

            let mut req_builder = self
                .http_client
                .post(self.messages_url())
                .header("Authorization", format!("Bearer {}", self.anthropic_token))
                .header("anthropic-version", "2023-06-01");
            for (k, v) in &self.anthropic_extra_headers {
                req_builder = req_builder.header(k.as_str(), v.as_str());
            }
            let response = req_builder
                .json(&request)
                .send()
                .await
                .map_err(AgentError::Http)?
                .json::<AnthropicResponse>()
                .await
                .map_err(AgentError::Http)?;

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

            let mut req_builder = self
                .http_client
                .post(self.messages_url())
                .header("Authorization", format!("Bearer {}", self.anthropic_token))
                .header("anthropic-version", "2023-06-01");
            for (k, v) in &self.anthropic_extra_headers {
                req_builder = req_builder.header(k.as_str(), v.as_str());
            }
            let response = req_builder
                .json(&request)
                .send()
                .await
                .map_err(AgentError::Http)?
                .json::<AnthropicResponse>()
                .await
                .map_err(AgentError::Http)?;

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
    pub async fn run_chat_streaming(
        &self,
        initial_messages: Vec<Message>,
        tools: &[ToolDef],
        system_prompt: Option<&str>,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Vec<Message>, AgentError> {
        let mut messages = initial_messages;

        let mut all_tools: Vec<ToolDef> = tools.to_vec();
        all_tools.extend(self.mcp_tool_defs.iter().cloned());
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

            let mut req_builder = self
                .http_client
                .post(self.messages_url())
                .header("Authorization", format!("Bearer {}", self.anthropic_token))
                .header("anthropic-version", "2023-06-01");
            for (k, v) in &self.anthropic_extra_headers {
                req_builder = req_builder.header(k.as_str(), v.as_str());
            }
            let response = req_builder
                .json(&body)
                .send()
                .await
                .map_err(AgentError::Http)?
                .json::<AnthropicResponse>()
                .await
                .map_err(AgentError::Http)?;

            if let Some(ref u) = response.usage {
                total_input = total_input.saturating_add(u.input_tokens);
                total_output = total_output.saturating_add(u.output_tokens);
                total_cache_creation =
                    total_cache_creation.saturating_add(u.cache_creation_input_tokens);
                total_cache_read = total_cache_read.saturating_add(u.cache_read_input_tokens);
            }

            match response.stop_reason.as_str() {
                "end_turn" => {
                    // Emit thinking blocks before text
                    for block in &response.content {
                        if let ContentBlock::Thinking { thinking } = block {
                            let _ = event_tx
                                .send(AgentEvent::ThinkingDelta {
                                    text: thinking.clone(),
                                })
                                .await;
                        }
                    }

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

                    let _ = event_tx
                        .send(AgentEvent::UsageSummary {
                            input_tokens: total_input,
                            output_tokens: total_output,
                            cache_creation_tokens: total_cache_creation,
                            cache_read_tokens: total_cache_read,
                        })
                        .await;
                    let _ = event_tx.send(AgentEvent::TextDelta { text }).await;

                    messages.push(Message::assistant(response.content));
                    info!(iterations = iteration + 1, "Streaming chat completed");
                    return Ok(messages);
                }
                "max_tokens" => {
                    // Emit whatever partial text was in the response before signalling
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
                    let _ = event_tx
                        .send(AgentEvent::UsageSummary {
                            input_tokens: total_input,
                            output_tokens: total_output,
                            cache_creation_tokens: total_cache_creation,
                            cache_read_tokens: total_cache_read,
                        })
                        .await;
                    Self::emit_partial_text(&event_tx, text).await;
                    warn!(iteration, "Streaming chat hit max_tokens (context full)");
                    return Err(AgentError::MaxTokens);
                }
                "tool_use" => {
                    let results = self
                        .execute_tools_streaming(&response.content, &event_tx)
                        .await;
                    messages.push(Message::assistant(response.content));
                    messages.push(Message::tool_results(results));
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

    /// Sends a [`AgentEvent::TextDelta`] when `text` is non-empty.
    /// Extracted to allow `#[coverage(off)]` — the closing `}` of an async
    /// `if` block is an LLVM coverage artifact in state-machine code.
    #[cfg_attr(coverage, coverage(off))]
    async fn emit_partial_text(event_tx: &tokio::sync::mpsc::Sender<AgentEvent>, text: String) {
        if !text.is_empty() {
            let _ = event_tx.send(AgentEvent::TextDelta { text }).await;
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn execute_tools_streaming(
        &self,
        content: &[ContentBlock],
        event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Vec<ToolResult> {
        let mut results = Vec::new();

        for block in content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, "Executing tool (streaming)");

                let _ = event_tx
                    .send(AgentEvent::ToolCallStarted {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    })
                    .await;

                // Ask permission before executing (if a checker is installed)
                let allowed = match &self.permission_checker {
                    Some(checker) => checker.check(id, name, input).await,
                    None => true,
                };

                let output = if !allowed {
                    format!("Permission denied: user refused to run tool `{name}`")
                } else if let Some((_, original, client)) = self
                    .mcp_dispatch
                    .iter()
                    .find(|(prefixed, _, _)| prefixed == name)
                {
                    match client.call_tool(original, input).await {
                        Ok(out) => out,
                        Err(e) => format!("Tool error: {e}"),
                    }
                } else {
                    dispatch_tool(&self.tool_context, name, input).await
                };

                let _ = event_tx
                    .send(AgentEvent::ToolCallFinished {
                        id: id.clone(),
                        output: output.clone(),
                        exit_code: None,
                        signal: None,
                    })
                    .await;

                results.push(ToolResult {
                    tool_use_id: id.clone(),
                    content: output,
                });
            }
        }

        results
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn execute_tools(&self, content: &[ContentBlock]) -> Vec<ToolResult> {
        let mut results = Vec::new();

        for block in content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, "Executing tool");

                // Check MCP dispatch first, then fall back to built-in tools.
                let output = if let Some((_, original, client)) = self
                    .mcp_dispatch
                    .iter()
                    .find(|(prefixed, _, _)| prefixed == name)
                {
                    match client.call_tool(original, input).await {
                        Ok(out) => out,
                        Err(e) => format!("Tool error: {e}"),
                    }
                } else {
                    dispatch_tool(&self.tool_context, name, input).await
                };

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
        });
        AgentLoop {
            http_client,
            proxy_url: "http://unused:9999".to_string(),
            anthropic_token: "test".to_string(),
            anthropic_base_url: None,
            anthropic_extra_headers: vec![],
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
        }];
        let results = agent.execute_tools(&content).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Unknown tool"));
    }

    /// Covers lines 685-686 (MCP Ok arm) in execute_tools_streaming.
    #[tokio::test]
    async fn execute_tools_streaming_mcp_ok_covers_ok_arm() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/mcp");
                then.status(200).body(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"mcp ok"}],"isError":false}}"#,
                );
            })
            .await;
        let http = reqwest::Client::new();
        let client = Arc::new(trogon_mcp::McpClient::new(http, server.url("/mcp")));
        let mut agent = make_test_agent();
        agent.mcp_dispatch = vec![("srv__tool".to_string(), "tool".to_string(), client)];
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "m1".to_string(),
            name: "srv__tool".to_string(),
            input: serde_json::json!({}),
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;
        assert_eq!(results[0].content, "mcp ok");
    }

    /// Covers line 687 (MCP Err arm) in execute_tools_streaming.
    #[tokio::test]
    async fn execute_tools_streaming_mcp_err_covers_err_arm() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/mcp");
                then.status(200).body(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"tool failed"}],"isError":true}}"#,
                );
            })
            .await;
        let http = reqwest::Client::new();
        let client = Arc::new(trogon_mcp::McpClient::new(http, server.url("/mcp")));
        let mut agent = make_test_agent();
        agent.mcp_dispatch = vec![("srv__tool2".to_string(), "tool2".to_string(), client)];
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let content = vec![ContentBlock::ToolUse {
            id: "m2".to_string(),
            name: "srv__tool2".to_string(),
            input: serde_json::json!({}),
        }];
        let results = agent.execute_tools_streaming(&content, &tx).await;
        assert!(results[0].content.contains("Tool error"));
    }

    /// Covers lines 725-726 (MCP Ok arm) in execute_tools.
    #[tokio::test]
    async fn execute_tools_mcp_ok_covers_ok_arm() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/mcp");
                then.status(200).body(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"sync ok"}],"isError":false}}"#,
                );
            })
            .await;
        let http = reqwest::Client::new();
        let client = Arc::new(trogon_mcp::McpClient::new(http, server.url("/mcp")));
        let mut agent = make_test_agent();
        agent.mcp_dispatch = vec![("s__t".to_string(), "t".to_string(), client)];
        let content = vec![ContentBlock::ToolUse {
            id: "m3".to_string(),
            name: "s__t".to_string(),
            input: serde_json::json!({}),
        }];
        let results = agent.execute_tools(&content).await;
        assert_eq!(results[0].content, "sync ok");
    }

    /// Covers line 727 (MCP Err arm) in execute_tools.
    #[tokio::test]
    async fn execute_tools_mcp_err_covers_err_arm() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/mcp");
                then.status(200).body(
                    r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"sync fail"}],"isError":true}}"#,
                );
            })
            .await;
        let http = reqwest::Client::new();
        let client = Arc::new(trogon_mcp::McpClient::new(http, server.url("/mcp")));
        let mut agent = make_test_agent();
        agent.mcp_dispatch = vec![("s__t2".to_string(), "t2".to_string(), client)];
        let content = vec![ContentBlock::ToolUse {
            id: "m4".to_string(),
            name: "s__t2".to_string(),
            input: serde_json::json!({}),
        }];
        let results = agent.execute_tools(&content).await;
        assert!(results[0].content.contains("Tool error"));
    }

    /// Covers line 629: TextDelta emitted in the max_tokens path when text is non-empty.
    #[tokio::test]
    async fn run_chat_streaming_max_tokens_with_text_emits_text_delta() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST);
                then.status(200).body(
                    r#"{"stop_reason":"max_tokens","content":[{"type":"text","text":"partial"}],"usage":{"input_tokens":10,"output_tokens":5}}"#,
                );
            })
            .await;
        let http = reqwest::Client::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let tool_context = Arc::new(crate::tools::ToolContext {
            http_client: http.clone(),
            proxy_url: server.url(""),
        });
        let agent = AgentLoop {
            http_client: http,
            proxy_url: server.url(""),
            anthropic_token: "test".to_string(),
            anthropic_base_url: None,
            anthropic_extra_headers: vec![],
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
        };
        let result = agent
            .run_chat_streaming(vec![Message::user_text("hello")], &[], None, tx)
            .await;
        assert!(result.is_err());
        let mut events = vec![];
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TextDelta { text } if text == "partial")),
            "expected TextDelta with 'partial' text, got: {events:?}"
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
}
