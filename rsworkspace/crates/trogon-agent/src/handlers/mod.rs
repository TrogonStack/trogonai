//! NATS event handlers — one module per integration.
//!
//! Each handler receives a raw NATS message payload, extracts the relevant
//! context, runs the agentic loop, and returns the final model output.

pub mod issue_triage;
pub mod pr_review;

use std::sync::Arc;

use crate::agent_loop::{AgentLoop, AgentError, Message};
use crate::tools::{ToolContext, ToolDef};

/// Common entry point used by both handlers.
///
/// Builds the [`AgentLoop`], constructs the initial user message, and runs
/// the loop.  Returns the text produced by the model.
pub async fn run_agent(
    agent: &AgentLoop,
    prompt: String,
    tools: Vec<ToolDef>,
) -> Result<String, AgentError> {
    let messages = vec![Message::user_text(prompt)];
    agent.run(messages, &tools).await
}

/// Convenience: build the shared [`ToolContext`] that all handlers share.
pub fn make_tool_context(
    http_client: reqwest::Client,
    proxy_url: String,
    github_token: String,
    linear_token: String,
) -> Arc<ToolContext> {
    Arc::new(ToolContext { http_client, proxy_url, github_token, linear_token })
}
