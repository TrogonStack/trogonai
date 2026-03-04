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

// ── Wire types ────────────────────────────────────────────────────────────────

/// A single message in the Anthropic conversation history.
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    role: &'static str,
    content: Vec<ContentBlock>,
}

impl Message {
    /// Simple user turn with plain text.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user",
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// Assistant turn (used when appending a model response to history).
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self { role: "assistant", content }
    }

    /// User turn carrying `tool_result` blocks.
    pub fn tool_results(results: Vec<ToolResult>) -> Self {
        Self {
            role: "user",
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
    ToolUse { id: String, name: String, input: Value },
    /// Result returned to the model after executing a tool.
    ToolResult { tool_use_id: String, content: String },
}

/// Pair of tool-use ID and the string result to feed back to the model.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
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
        if let Self::Http(e) = self { Some(e) } else { None }
    }
}

// ── AgentLoop ─────────────────────────────────────────────────────────────────

/// Runs the Anthropic tool-use loop, routing all AI calls through the proxy.
pub struct AgentLoop {
    pub http_client: reqwest::Client,
    /// Base URL of the running `trogon-secret-proxy`.
    pub proxy_url: String,
    /// Opaque proxy token for Anthropic (never the real API key).
    pub anthropic_token: String,
    pub model: String,
    pub max_iterations: u32,
    /// Shared context passed to every tool execution.
    pub tool_context: Arc<ToolContext>,
}

impl AgentLoop {
    /// Run the agentic loop starting from `initial_messages`.
    ///
    /// Returns the final text produced by the model when it stops requesting
    /// tools.
    pub async fn run(
        &self,
        initial_messages: Vec<Message>,
        tools: &[ToolDef],
    ) -> Result<String, AgentError> {
        let mut messages = initial_messages;

        for iteration in 0..self.max_iterations {
            debug!(iteration, "Agent loop iteration");

            let request = AnthropicRequest {
                model: &self.model,
                max_tokens: 4096,
                tools,
                messages: &messages,
            };

            let response = self
                .http_client
                .post(format!("{}/anthropic/v1/messages", self.proxy_url))
                .header(
                    "Authorization",
                    format!("Bearer {}", self.anthropic_token),
                )
                .header("anthropic-version", "2023-06-01")
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

    async fn execute_tools(&self, content: &[ContentBlock]) -> Vec<ToolResult> {
        let mut results = Vec::new();

        for block in content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, "Executing tool");
                let output = dispatch_tool(&self.tool_context, name, input).await;
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
        assert!(AgentError::MaxIterationsReached
            .to_string()
            .contains("max iterations"));
        assert!(AgentError::UnexpectedStopReason("pause".to_string())
            .to_string()
            .contains("pause"));
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
        assert!(
            std::error::Error::source(&AgentError::MaxIterationsReached).is_none()
        );
    }
}
